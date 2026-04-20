//! Port-forward tunnels: per-local-port lock, ssh `-L` child process,
//! reconnect loop.
//!
//! A tunnel is scoped to a local port. At most one process may hold the
//! lock for a given local port at any time; this prevents two lzsync
//! instances from both trying to bind the same port (the second would lose
//! to `EADDRINUSE` anyway, but the lock gives the user a clean diagnostic
//! up front).
//!
//! The lock file is a small JSON blob under
//! `~/.config/lzsync/tunnel-locks/<local_port>.lock` containing the pid and
//! started_at of the owning process. The port number is already a small
//! unique key, so unlike the watch lock (which hashes a filesystem path),
//! we use the port directly in the filename.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::UnboundedSender;

use crate::target::{Target, TunnelConfig};
use crate::watch::is_pid_alive;

#[derive(Debug, Serialize, Deserialize)]
struct LockRecord {
    pid: u32,
    started_at: u64,
    /// Local port being forwarded — recorded for human diagnosis when a
    /// lock file is inspected directly. Not used for logic.
    local_port: u16,
}

/// Owned lock on a local port. Dropping the handle removes the lock file
/// (or leaves it if it's been overwritten by another process since
/// acquisition).
#[derive(Debug)]
pub struct LockHandle {
    #[allow(dead_code)] // retained for logging / diagnostics
    pub tunnel_name: String,
    pub lock_path: PathBuf,
    pub owner_pid: u32,
    released: bool,
}

impl LockHandle {
    /// Release the lock explicitly. Equivalent to dropping the handle but
    /// surfaces I/O errors.
    #[allow(dead_code)] // kept as explicit API surface alongside Drop
    pub fn release(mut self) -> anyhow::Result<()> {
        self.released = true;
        match fs::read_to_string(&self.lock_path) {
            Ok(txt) => {
                if let Ok(rec) = serde_json::from_str::<LockRecord>(&txt)
                    && rec.pid == self.owner_pid
                {
                    let _ = fs::remove_file(&self.lock_path);
                }
                Ok(())
            }
            Err(_) => Ok(()),
        }
    }
}

impl Drop for LockHandle {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        // Best-effort cleanup; only remove the file if we still own it.
        if let Ok(txt) = fs::read_to_string(&self.lock_path)
            && let Ok(rec) = serde_json::from_str::<LockRecord>(&txt)
            && rec.pid == self.owner_pid
        {
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}

#[derive(Debug, Clone)]
pub enum LockError {
    /// Another live process holds this lock. Returns its pid.
    Held { pid: u32 },
    /// Filesystem / serialization failure.
    Io(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Held { pid } => write!(f, "locked by PID {pid}"),
            LockError::Io(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for LockError {}

/// Try to take the tunnel lock for `local_port`. On success returns a
/// handle that releases the lock when dropped. On `LockError::Held` the
/// caller can display the offending PID to the user.
#[allow(dead_code)] // wired up by app.rs in the UI stage
pub fn try_acquire_lock(
    tunnel_name: &str,
    local_port: u16,
) -> std::result::Result<LockHandle, LockError> {
    try_acquire_lock_in(tunnel_name, local_port, &default_lock_dir())
}

fn try_acquire_lock_in(
    tunnel_name: &str,
    local_port: u16,
    lock_dir: &Path,
) -> std::result::Result<LockHandle, LockError> {
    fs::create_dir_all(lock_dir).map_err(|e| LockError::Io(format!("mkdir {lock_dir:?}: {e}")))?;
    let lock_path = lock_dir.join(format!("{local_port}.lock"));
    let pid = std::process::id();
    let record = LockRecord {
        pid,
        started_at: now_epoch(),
        local_port,
    };
    let encoded =
        serde_json::to_string(&record).map_err(|e| LockError::Io(format!("encode lock: {e}")))?;

    // Atomic claim: create_new fails if the file already exists.
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(mut f) => {
            f.write_all(encoded.as_bytes())
                .map_err(|e| LockError::Io(format!("write lock: {e}")))?;
            Ok(LockHandle {
                tunnel_name: tunnel_name.to_string(),
                lock_path,
                owner_pid: pid,
                released: false,
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Lock exists; check if the owner is alive.
            let txt = fs::read_to_string(&lock_path)
                .map_err(|e| LockError::Io(format!("read existing lock: {e}")))?;
            let existing: LockRecord = match serde_json::from_str(&txt) {
                Ok(r) => r,
                Err(_) => {
                    // Garbage lock file: overwrite it.
                    fs::write(&lock_path, &encoded)
                        .map_err(|e| LockError::Io(format!("overwrite corrupt lock: {e}")))?;
                    return Ok(LockHandle {
                        tunnel_name: tunnel_name.to_string(),
                        lock_path,
                        owner_pid: pid,
                        released: false,
                    });
                }
            };
            if is_pid_alive(existing.pid) {
                return Err(LockError::Held { pid: existing.pid });
            }
            // Stale lock — take over.
            fs::write(&lock_path, &encoded)
                .map_err(|e| LockError::Io(format!("steal stale lock: {e}")))?;
            Ok(LockHandle {
                tunnel_name: tunnel_name.to_string(),
                lock_path,
                owner_pid: pid,
                released: false,
            })
        }
        Err(e) => Err(LockError::Io(format!("create lock: {e}"))),
    }
}

#[allow(dead_code)] // wired up by app.rs in the UI stage
pub fn default_lock_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("lzsync/tunnel-locks"))
        .unwrap_or_else(|| PathBuf::from(".lzsync/tunnel-locks"))
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// =============================================================================
// Tunnel status / events
// =============================================================================

/// Current state of a tunnel, as surfaced to the UI. The UI renders an icon
/// per variant (`○` off, `…` starting, `⇄` connected, `↻` reconnecting,
/// `⚠` locked, `✗` failed).
#[derive(Debug, Clone)]
#[allow(dead_code)] // variants consumed by the UI in later stages
pub enum TunnelStatus {
    Off,
    Starting,
    Connected,
    Reconnecting {
        attempt: u32,
        next_retry_in: Duration,
    },
    /// Non-connection failure: config error, target missing, ssh binary
    /// absent. Terminal — the loop does not retry.
    Failed(String),
    /// Another live process holds this local port's lock.
    LockedByOther(u32),
}

/// Event emitted by a running tunnel task. Consumers observe these via the
/// `tx` channel passed to `start_tunnel`.
#[derive(Debug, Clone)]
#[allow(dead_code)] // variants consumed by the UI in later stages
pub enum TunnelEvent {
    StatusChanged {
        name: String,
        status: TunnelStatus,
    },
    /// One line of ssh stderr. Also mirrored to `transfer::log_line` so
    /// debugging is available from the transfer log.
    Stderr {
        name: String,
        line: String,
    },
    /// The tunnel task has exited (user toggled off, or non-retryable
    /// failure). The lock is released at this point.
    Stopped {
        name: String,
    },
}

/// Handle returned by `start_tunnel`. Dropping or calling `stop()` signals
/// the background task to exit on its next tick, which kills the ssh child
/// (via `kill_on_drop`) and releases the lock.
#[derive(Debug)]
#[allow(dead_code)] // consumed by app.rs in the UI stage
pub struct TunnelHandle {
    pub name: String,
    stop: Arc<AtomicBool>,
}

impl TunnelHandle {
    #[allow(dead_code)]
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for TunnelHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// =============================================================================
// stderr classification
// =============================================================================

/// Patterns that indicate "retrying won't help" — bail out instead of
/// entering the reconnect loop. These are all verbatim substrings that
/// OpenSSH prints on stderr.
pub(crate) fn is_fatal_stderr(line: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "Address already in use",
        "cannot listen to port",
        "Permission denied (publickey",
        "Permission denied, please try again",
        "Could not resolve hostname",
        "Host key verification failed",
        "No such file or directory",
    ];
    PATTERNS.iter().any(|p| line.contains(p))
}

/// Friendly label for a fatal stderr line. Keeps the local port in the
/// message when relevant so the user sees *which* port is stuck.
pub(crate) fn classify_fatal(line: &str, local_port: u16) -> String {
    if line.contains("Address already in use") || line.contains("cannot listen to port") {
        format!("local port {local_port} already in use")
    } else if line.contains("Permission denied") {
        "ssh auth failed (publickey/password)".into()
    } else if line.contains("Could not resolve hostname") {
        "unknown host".into()
    } else if line.contains("Host key verification failed") {
        "host key verification failed".into()
    } else if line.contains("No such file or directory") {
        "ssh key or config file missing".into()
    } else {
        line.to_string()
    }
}

// =============================================================================
// ssh argument construction
// =============================================================================

/// Build the full `ssh` argument vector for a `-L` forward, in a stable
/// order so tests can assert on the exact flag layout.
pub(crate) fn build_ssh_args(cfg: &TunnelConfig, target: &Target) -> Vec<String> {
    let mut args: Vec<String> = Vec::with_capacity(16);
    args.push("-N".into());
    args.push("-o".into());
    args.push("ServerAliveInterval=30".into());
    args.push("-o".into());
    args.push("ServerAliveCountMax=3".into());
    args.push("-o".into());
    args.push("ExitOnForwardFailure=yes".into());
    args.push("-o".into());
    args.push("StrictHostKeyChecking=accept-new".into());
    args.push("-p".into());
    args.push(target.ssh_port().to_string());
    if let Some(key) = &target.ssh_key {
        let expanded = shellexpand::tilde(key).into_owned();
        args.push("-i".into());
        args.push(expanded);
    }
    args.push("-L".into());
    args.push(format!(
        "{}:{}:{}:{}",
        cfg.bind_address, cfg.local_port, cfg.remote_host, cfg.remote_port
    ));
    let user_host = match &target.user {
        Some(u) if !u.is_empty() => format!("{u}@{}", target.host),
        _ => target.host.clone(),
    };
    args.push(user_host);
    args
}

/// Exponential backoff schedule: 1s → 2s → 5s → 10s → 30s, capped at 30s.
pub(crate) fn next_backoff(current: Duration) -> Duration {
    let secs = current.as_secs();
    let next = if secs < 2 {
        2
    } else if secs < 5 {
        5
    } else if secs < 10 {
        10
    } else {
        30
    };
    Duration::from_secs(next)
}

// =============================================================================
// ssh process driver
// =============================================================================

/// Spawn the background task that owns the lock, the ssh child process,
/// and the reconnect loop. Dropping the returned `TunnelHandle` stops the
/// task on its next tick (<=500ms), which kills the ssh child and releases
/// the lock.
#[allow(dead_code)] // wired up by app.rs in the UI stage
pub fn start_tunnel(
    cfg: TunnelConfig,
    target: Target,
    lock: LockHandle,
    tx: UnboundedSender<TunnelEvent>,
) -> TunnelHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);
    let name = cfg.name.clone();
    tokio::spawn(async move {
        let _owned_lock = lock; // released when this task exits
        tunnel_loop(cfg, target, tx, stop_clone).await;
    });
    TunnelHandle { name, stop }
}

async fn tunnel_loop(
    cfg: TunnelConfig,
    target: Target,
    tx: UnboundedSender<TunnelEvent>,
    stop: Arc<AtomicBool>,
) {
    let name = cfg.name.clone();
    let mut attempt: u32 = 1;
    let mut backoff = Duration::from_secs(1);
    // Set by the stderr drain when it sees a non-retryable error (e.g. local
    // port already in use). The main loop checks this after each ssh exit
    // and terminates instead of looping forever against a condition retries
    // can't fix.
    let fatal = Arc::new(AtomicBool::new(false));

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let _ = tx.send(TunnelEvent::StatusChanged {
            name: name.clone(),
            status: TunnelStatus::Starting,
        });

        let args = build_ssh_args(&cfg, &target);
        let mut child = match tokio::process::Command::new("ssh")
            .args(&args)
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(TunnelEvent::StatusChanged {
                    name: name.clone(),
                    status: TunnelStatus::Failed(format!("spawn ssh: {e}")),
                });
                break;
            }
        };

        // Drain stderr in a sibling task; each line flows to the UI AND
        // the transfer log so debugging is available offline. Also watches
        // for non-retryable patterns (e.g. local bind conflict) and flips
        // the `fatal` flag so the main loop can stop instead of retrying.
        if let Some(err) = child.stderr.take() {
            let tx_err = tx.clone();
            let name_err = name.clone();
            let fatal_err = Arc::clone(&fatal);
            let local_port = cfg.local_port;
            tokio::spawn(async move {
                let mut reader = BufReader::new(err).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    crate::transfer::log_line(&format!("tunnel[{name_err}] {line}"));
                    if is_fatal_stderr(&line) {
                        fatal_err.store(true, Ordering::Relaxed);
                        let _ = tx_err.send(TunnelEvent::StatusChanged {
                            name: name_err.clone(),
                            status: TunnelStatus::Failed(classify_fatal(&line, local_port)),
                        });
                    }
                    let _ = tx_err.send(TunnelEvent::Stderr {
                        name: name_err.clone(),
                        line,
                    });
                }
            });
        }

        // Optimistic: with `-ExitOnForwardFailure=yes` ssh exits immediately
        // on bind failure, so if it's still alive after spawn we're forwarding.
        let _ = tx.send(TunnelEvent::StatusChanged {
            name: name.clone(),
            status: TunnelStatus::Connected,
        });

        // Wait for the child to exit OR for the stop flag to flip. Poll the
        // flag at 500ms intervals so teardown is responsive.
        let exit_was_intentional = loop {
            if stop.load(Ordering::Relaxed) {
                let _ = child.kill().await;
                // Reap the exit status so the Child drop doesn't warn.
                let _ = child.wait().await;
                break true;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
                _ = child.wait() => break false,
            }
        };

        if exit_was_intentional || stop.load(Ordering::Relaxed) {
            break;
        }

        // Non-retryable error surfaced by the stderr drain — stop looping.
        // The drain already emitted the Failed status; we just exit.
        if fatal.load(Ordering::Relaxed) {
            break;
        }

        // Unintended exit — announce reconnect, then sleep in short ticks
        // so the user can cancel without waiting out the full backoff.
        let _ = tx.send(TunnelEvent::StatusChanged {
            name: name.clone(),
            status: TunnelStatus::Reconnecting {
                attempt,
                next_retry_in: backoff,
            },
        });
        let until = Instant::now() + backoff;
        while Instant::now() < until {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        attempt = attempt.saturating_add(1);
        backoff = next_backoff(backoff);
    }

    let _ = tx.send(TunnelEvent::Stopped { name });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // =====================
    // Lock module
    // =====================

    #[test]
    fn atomic_create_returns_handle() {
        let tmp = TempDir::new().unwrap();
        let lock_dir = tmp.path().join("locks");
        let h = try_acquire_lock_in("jupyter", 8888, &lock_dir).unwrap();
        assert_eq!(h.owner_pid, std::process::id());
        assert!(h.lock_path.exists());
    }

    #[test]
    fn second_acquire_returns_held_when_alive() {
        let tmp = TempDir::new().unwrap();
        let lock_dir = tmp.path().join("locks");
        let _first = try_acquire_lock_in("jupyter", 8888, &lock_dir).unwrap();
        match try_acquire_lock_in("jupyter", 8888, &lock_dir) {
            Err(LockError::Held { pid }) => assert_eq!(pid, std::process::id()),
            other => panic!("expected Held, got {other:?}"),
        }
    }

    #[test]
    fn stale_lock_is_reclaimed() {
        let tmp = TempDir::new().unwrap();
        let lock_dir = tmp.path().join("locks");
        fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join("8888.lock");
        let stale = LockRecord {
            pid: 999_999,
            started_at: 0,
            local_port: 8888,
        };
        fs::write(&lock_path, serde_json::to_string(&stale).unwrap()).unwrap();

        let h = try_acquire_lock_in("jupyter", 8888, &lock_dir).unwrap();
        assert_eq!(h.owner_pid, std::process::id());
    }

    #[test]
    fn drop_removes_our_lock() {
        let tmp = TempDir::new().unwrap();
        let lock_dir = tmp.path().join("locks");
        let lock_path;
        {
            let h = try_acquire_lock_in("jupyter", 8888, &lock_dir).unwrap();
            lock_path = h.lock_path.clone();
            assert!(lock_path.exists());
        }
        assert!(!lock_path.exists(), "Drop should clean up our own lock");
    }

    #[test]
    fn corrupt_lock_is_overwritten() {
        let tmp = TempDir::new().unwrap();
        let lock_dir = tmp.path().join("locks");
        fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join("8888.lock");
        fs::write(&lock_path, "not json at all").unwrap();
        let h = try_acquire_lock_in("jupyter", 8888, &lock_dir).unwrap();
        assert_eq!(h.owner_pid, std::process::id());
    }

    // =====================
    // ssh argument construction
    // =====================

    fn test_tunnel_cfg() -> TunnelConfig {
        TunnelConfig {
            name: "jupyter".into(),
            target: "mybox".into(),
            local_port: 8888,
            remote_host: "localhost".into(),
            remote_port: 8888,
            bind_address: "127.0.0.1".into(),
            autostart: false,
        }
    }

    fn test_target() -> Target {
        Target {
            name: "mybox".into(),
            host: "mybox.example.com".into(),
            user: Some("alice".into()),
            remote_dir: "/tmp".into(),
            ssh_port: Some(2222),
            ssh_key: Some("~/.ssh/id_ed25519".into()),
            clipboard_format: None,
        }
    }

    #[test]
    fn ssh_args_contain_expected_flags_with_key_and_user() {
        let cfg = test_tunnel_cfg();
        let target = test_target();
        let args = build_ssh_args(&cfg, &target);

        // All non-negotiable flags present.
        assert!(args.contains(&"-N".to_string()));
        assert!(args.contains(&"ServerAliveInterval=30".to_string()));
        assert!(args.contains(&"ServerAliveCountMax=3".to_string()));
        assert!(args.contains(&"ExitOnForwardFailure=yes".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=accept-new".to_string()));

        // Port wiring.
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"2222".to_string()));

        // Key expansion — `~` must be resolved.
        assert!(args.contains(&"-i".to_string()));
        let expanded = shellexpand::tilde("~/.ssh/id_ed25519").into_owned();
        assert!(
            args.contains(&expanded),
            "expected expanded key path in args: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.starts_with("~")),
            "no arg should still contain a literal `~`: {args:?}"
        );

        // Forward spec.
        assert!(args.contains(&"-L".to_string()));
        assert!(
            args.contains(&"127.0.0.1:8888:localhost:8888".to_string()),
            "expected forward spec in args: {args:?}"
        );

        // user@host always last.
        assert_eq!(args.last().unwrap(), "alice@mybox.example.com");
    }

    #[test]
    fn ssh_args_omit_user_prefix_when_missing() {
        let cfg = test_tunnel_cfg();
        let mut target = test_target();
        target.user = None;
        let args = build_ssh_args(&cfg, &target);
        assert_eq!(args.last().unwrap(), "mybox.example.com");
        assert!(
            !args.iter().any(|a| a.contains('@')),
            "no arg should contain '@' when user is None: {args:?}"
        );
    }

    #[test]
    fn ssh_args_omit_key_when_none() {
        let cfg = test_tunnel_cfg();
        let mut target = test_target();
        target.ssh_key = None;
        let args = build_ssh_args(&cfg, &target);
        assert!(
            !args.contains(&"-i".to_string()),
            "-i flag must be absent when ssh_key is None: {args:?}"
        );
    }

    // =====================
    // Backoff schedule
    // =====================

    #[test]
    fn next_backoff_climbs_then_caps() {
        assert_eq!(next_backoff(Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(next_backoff(Duration::from_secs(2)), Duration::from_secs(5));
        assert_eq!(
            next_backoff(Duration::from_secs(5)),
            Duration::from_secs(10)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(10)),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }

    // =====================
    // stderr classifier
    // =====================

    #[test]
    fn fatal_detects_bind_address_already_in_use() {
        assert!(is_fatal_stderr(
            "bind [127.0.0.1]:57988: Address already in use"
        ));
        assert!(is_fatal_stderr(
            "channel_setup_fwd_listener_tcpip: cannot listen to port: 57988"
        ));
    }

    #[test]
    fn fatal_detects_auth_failure() {
        assert!(is_fatal_stderr(
            "Permission denied (publickey,password,keyboard-interactive)."
        ));
    }

    #[test]
    fn fatal_detects_dns_failure() {
        assert!(is_fatal_stderr(
            "ssh: Could not resolve hostname nosuchhost.local: nodename nor servname provided"
        ));
    }

    #[test]
    fn fatal_ignores_transient_noise() {
        assert!(!is_fatal_stderr(
            "channel 2: open failed: connect failed: Connection refused"
        ));
        assert!(!is_fatal_stderr(
            "Warning: Permanently added 'host' (ED25519)"
        ));
    }

    #[test]
    fn classify_fatal_mentions_local_port_for_bind_conflict() {
        let msg = classify_fatal("bind [127.0.0.1]:57988: Address already in use", 57988);
        assert!(msg.contains("57988"));
        assert!(msg.contains("already in use"));
    }

    #[test]
    fn classify_fatal_labels_auth_failure() {
        let msg = classify_fatal("Permission denied (publickey).", 8888);
        assert!(msg.contains("auth"));
    }
}
