use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::target::Target;

#[derive(Debug, Clone)]
#[allow(dead_code)] // Several fields are reserved for UI features in later versions.
pub enum TransferEvent {
    Started {
        id: u64,
        target_name: String,
        local: PathBuf,
    },
    Progress {
        id: u64,
        bytes: u64,
        percent: u8,
        rate: String,
    },
    Line {
        id: u64,
        line: String,
    },
    Completed {
        id: u64,
        remote_abs_dir: String,
    },
    Failed {
        id: u64,
        error: String,
    },
}

pub struct Transfer {
    #[allow(dead_code)]
    pub id: u64,
    pub target_name: String,
    pub local: PathBuf,
    pub remote_abs_dir: String,
    pub percent: u8,
    pub rate: String,
    pub state: TransferState,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    Pending,
    Running,
    Completed,
    Failed,
}

impl Transfer {
    pub fn new(id: u64, target: &Target, local: PathBuf) -> Self {
        Self {
            id,
            target_name: target.name.clone(),
            local,
            remote_abs_dir: String::new(),
            percent: 0,
            rate: String::new(),
            state: TransferState::Pending,
            last_error: None,
        }
    }
}

/// Spawn rsync for `local` → `target`. Events are emitted via `tx`.
pub fn spawn(id: u64, target: Target, local: PathBuf, tx: mpsc::UnboundedSender<TransferEvent>) {
    tokio::spawn(async move {
        if let Err(e) = run(id, &target, &local, &tx).await {
            let _ = tx.send(TransferEvent::Failed {
                id,
                error: format!("{e:#}"),
            });
        }
    });
}

async fn run(
    id: u64,
    target: &Target,
    local: &Path,
    tx: &mpsc::UnboundedSender<TransferEvent>,
) -> Result<()> {
    let _ = tx.send(TransferEvent::Started {
        id,
        target_name: target.name.clone(),
        local: local.to_path_buf(),
    });

    let remote_abs_dir = resolve_remote_home(target)
        .await
        .with_context(|| format!("resolving remote dir for target '{}'", target.name))?;

    let endpoint = match &target.user {
        Some(u) => format!("{u}@{}:{}/", target.host, remote_abs_dir),
        None => format!("{}:{}/", target.host, remote_abs_dir),
    };

    let mut ssh_opt = format!("ssh -p {}", target.ssh_port());
    if let Some(key) = &target.ssh_key {
        let expanded = shellexpand::tilde(key);
        ssh_opt.push_str(&format!(" -i {expanded}"));
    }
    // Keep rsync non-interactive.
    ssh_opt.push_str(" -o BatchMode=no -o StrictHostKeyChecking=accept-new");

    let mut cmd = Command::new("rsync");
    cmd.arg("--info=progress2")
        .arg("--partial")
        .arg("-e")
        .arg(&ssh_opt)
        .arg(local)
        .arg(&endpoint)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawning rsync")?;
    let stdout = child.stdout.take().context("rsync stdout pipe")?;
    let stderr = child.stderr.take().context("rsync stderr pipe")?;

    let tx_out = tx.clone();
    let tx_err = tx.clone();
    let stdout_task = tokio::spawn(async move {
        read_progress_stream(id, stdout, tx_out).await;
    });
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_err.send(TransferEvent::Line { id, line });
        }
    });

    let status = child.wait().await.context("rsync wait")?;
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if status.success() {
        let _ = tx.send(TransferEvent::Completed { id, remote_abs_dir });
        Ok(())
    } else {
        let code = status.code().unwrap_or(-1);
        let _ = tx.send(TransferEvent::Failed {
            id,
            error: format!("rsync exited with status {code}"),
        });
        Ok(())
    }
}

async fn read_progress_stream<R>(id: u64, stdout: R, tx: mpsc::UnboundedSender<TransferEvent>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    // rsync --info=progress2 uses \r to overwrite its progress line. Read
    // byte-by-byte and split on both \n and \r so we catch every update.
    use tokio::io::AsyncReadExt;
    let mut rdr = BufReader::new(stdout);
    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        match rdr.read(&mut byte).await {
            Ok(0) => break,
            Ok(_) => {
                let b = byte[0];
                if b == b'\n' || b == b'\r' {
                    if !buf.is_empty() {
                        let line = String::from_utf8_lossy(&buf).to_string();
                        emit_line(id, &line, &tx);
                        buf.clear();
                    }
                } else {
                    buf.push(b);
                }
            }
            Err(_) => break,
        }
    }
    if !buf.is_empty() {
        let line = String::from_utf8_lossy(&buf).to_string();
        emit_line(id, &line, &tx);
    }
}

fn emit_line(id: u64, line: &str, tx: &mpsc::UnboundedSender<TransferEvent>) {
    if let Some(p) = parse_progress_line(line) {
        let _ = tx.send(TransferEvent::Progress {
            id,
            bytes: p.bytes,
            percent: p.percent,
            rate: p.rate,
        });
    } else {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let _ = tx.send(TransferEvent::Line {
                id,
                line: trimmed.to_string(),
            });
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Progress {
    pub bytes: u64,
    pub percent: u8,
    pub rate: String,
}

/// Parse a line like:
/// `     1,234,567  45%   12.34MB/s    0:00:03`
pub fn parse_progress_line(line: &str) -> Option<Progress> {
    // Lazy static-ish — compile on first call.
    static RE_SRC: &str = r"(?x)
        ^\s*
        ([\d,]+)\s+                     # bytes (with commas)
        (\d{1,3})%\s+                   # percent
        ([\d.]+\s*[KMGT]?B/s)           # rate
    ";
    thread_local! {
        static RE: Regex = Regex::new(RE_SRC).expect("progress regex");
    }
    RE.with(|re| {
        let caps = re.captures(line)?;
        let bytes: u64 = caps[1].replace(',', "").parse().ok()?;
        let percent: u8 = caps[2].parse().ok()?;
        let rate = caps[3].to_string();
        Some(Progress {
            bytes,
            percent,
            rate,
        })
    })
}

/// Expand remote_dir's `~` / `$HOME` *and* create the directory on the remote.
/// Returns the absolute remote path.
async fn resolve_remote_home(target: &Target) -> Result<String> {
    // Single round-trip: let the remote shell expand $HOME, mkdir -p, print the
    // resolved absolute path. This both fixes "~/foo doesn't exist" and removes
    // a second ssh call on the hot path.
    let mut cmd = Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("-o").arg("ConnectTimeout=10");
    cmd.arg("-p").arg(target.ssh_port().to_string());
    if let Some(key) = &target.ssh_key {
        let expanded = shellexpand::tilde(key);
        cmd.arg("-i").arg(expanded.as_ref());
    }
    let addr = match &target.user {
        Some(u) => format!("{u}@{}", target.host),
        None => target.host.clone(),
    };
    cmd.arg(addr);
    // Use single quotes locally and escape the remote_dir value — the remote
    // shell expands ~ / $HOME.
    let script = format!(
        r#"d={remote}; d="${{d/#~/$HOME}}"; mkdir -p "$d" && cd "$d" && pwd"#,
        remote = shell_single_quote(&target.remote_dir)
    );
    cmd.arg(script);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = cmd.output().await.context("ssh mkdir -p")?;
    if !output.status.success() {
        anyhow::bail!(
            "ssh remote prep failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if resolved.is_empty() {
        anyhow::bail!("remote dir resolution returned empty");
    }
    Ok(resolved)
}

fn shell_single_quote(s: &str) -> String {
    // Wrap s in single quotes, escaping any embedded single quotes.
    // ' → '\''
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Ping one target: verify rsync + ssh connectivity. Used for UI status.
#[allow(dead_code)] // Wired up in a follow-up patch; kept here for 0.1.0.
pub async fn preflight(target: &Target) -> Result<()> {
    // Ensure rsync exists locally.
    let which = Command::new("sh")
        .arg("-c")
        .arg("command -v rsync >/dev/null 2>&1")
        .status()
        .await;
    if !matches!(which, Ok(s) if s.success()) {
        anyhow::bail!("rsync not installed locally");
    }
    // Verify ssh reachability with a short timeout, non-interactive.
    let mut cmd = Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("-o").arg("ConnectTimeout=5");
    cmd.arg("-o").arg("StrictHostKeyChecking=accept-new");
    cmd.arg("-p").arg(target.ssh_port().to_string());
    if let Some(key) = &target.ssh_key {
        let expanded = shellexpand::tilde(key);
        cmd.arg("-i").arg(expanded.as_ref());
    }
    let addr = match &target.user {
        Some(u) => format!("{u}@{}", target.host),
        None => target.host.clone(),
    };
    cmd.arg(addr).arg("true");
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let out = cmd.output().await.context("ssh preflight")?;
    if !out.status.success() {
        anyhow::bail!(
            "ssh unreachable: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Full preflight: ssh reachable + remote has rsync. Returns Ok on success,
/// Err with a message that the caller can pattern-match on
/// ("rsync not found on remote") to offer auto-install.
pub async fn preflight_full(target: &Target) -> Result<()> {
    // Local rsync check
    let local_ok = Command::new("sh")
        .arg("-c")
        .arg("command -v rsync >/dev/null 2>&1")
        .status()
        .await;
    if !matches!(local_ok, Ok(s) if s.success()) {
        anyhow::bail!("rsync not installed locally");
    }

    // Combined ssh probe: exit 0 means ssh works + rsync found. Exit 66 (arbitrary)
    // means ssh works but rsync is missing.
    let mut cmd = Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("-o").arg("ConnectTimeout=6");
    cmd.arg("-o").arg("StrictHostKeyChecking=accept-new");
    cmd.arg("-p").arg(target.ssh_port().to_string());
    if let Some(key) = &target.ssh_key {
        let expanded = shellexpand::tilde(key);
        cmd.arg("-i").arg(expanded.as_ref());
    }
    let addr = match &target.user {
        Some(u) => format!("{u}@{}", target.host),
        None => target.host.clone(),
    };
    cmd.arg(addr)
        .arg("if command -v rsync >/dev/null 2>&1; then exit 0; else exit 66; fi");
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let out = cmd.output().await.context("ssh preflight_full")?;
    match out.status.code() {
        Some(0) => Ok(()),
        Some(66) => anyhow::bail!("rsync not found on remote"),
        _ => {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if err.is_empty() {
                anyhow::bail!("ssh unreachable");
            } else {
                anyhow::bail!("ssh unreachable: {err}")
            }
        }
    }
}

/// Install rsync on the remote host. Detects the package manager via
/// /etc/os-release's ID field and runs the appropriate install command.
/// Uses `sudo -n` (non-interactive) — if sudo needs a password, installation
/// fails with a message suggesting manual install.
pub async fn remote_install_rsync(target: &Target) -> Result<()> {
    let mut cmd = Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("-o").arg("ConnectTimeout=10");
    cmd.arg("-p").arg(target.ssh_port().to_string());
    if let Some(key) = &target.ssh_key {
        let expanded = shellexpand::tilde(key);
        cmd.arg("-i").arg(expanded.as_ref());
    }
    let addr = match &target.user {
        Some(u) => format!("{u}@{}", target.host),
        None => target.host.clone(),
    };
    cmd.arg(addr).arg(
        // Shell script run on remote: pick the right package manager and install
        // rsync, preferring passwordless sudo; fall back to plain (already root).
        r#"set -e
                if command -v rsync >/dev/null 2>&1; then
                    exit 0
                fi
                sudo() { if [ "$(id -u)" = "0" ]; then "$@"; else command sudo -n "$@"; fi; }
                if command -v apt-get >/dev/null 2>&1; then
                    sudo apt-get update -qq && sudo apt-get install -y rsync
                elif command -v dnf >/dev/null 2>&1; then
                    sudo dnf install -y rsync
                elif command -v yum >/dev/null 2>&1; then
                    sudo yum install -y rsync
                elif command -v apk >/dev/null 2>&1; then
                    sudo apk add --no-cache rsync
                elif command -v pacman >/dev/null 2>&1; then
                    sudo pacman -Sy --noconfirm rsync
                elif command -v zypper >/dev/null 2>&1; then
                    sudo zypper install -y rsync
                elif command -v brew >/dev/null 2>&1; then
                    brew install rsync
                else
                    echo "no supported package manager found" >&2
                    exit 1
                fi
                command -v rsync >/dev/null 2>&1"#,
    );
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().await.context("ssh install")?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_progress_basic() {
        let p = parse_progress_line("     1,234,567  45%   12.34MB/s    0:00:03").expect("parses");
        assert_eq!(p.bytes, 1_234_567);
        assert_eq!(p.percent, 45);
        assert_eq!(p.rate, "12.34MB/s");
    }

    #[test]
    fn parse_progress_no_commas() {
        let p = parse_progress_line("   500  5%  1.00KB/s 0:00:02").expect("parses");
        assert_eq!(p.bytes, 500);
        assert_eq!(p.percent, 5);
    }

    #[test]
    fn parse_progress_complete() {
        let p = parse_progress_line("  12,345,678 100%   20.12MB/s    0:00:00 (xfr#1, to-chk=0/1)")
            .expect("parses");
        assert_eq!(p.percent, 100);
    }

    #[test]
    fn parse_progress_ignores_random_line() {
        assert!(parse_progress_line("sending incremental file list").is_none());
    }

    #[test]
    fn parse_progress_gbps_rate() {
        let p = parse_progress_line("     1,234  50%   1.25GB/s    0:00:01").expect("parses");
        assert_eq!(p.rate, "1.25GB/s");
    }
}
