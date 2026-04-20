use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};

const VERSION_URL: &str = "https://raw.githubusercontent.com/FanFusion/lzsync/main/VERSION";

/// Fetch remote VERSION. Returns Some(version) if newer than current, None if
/// same-or-older.
pub async fn check_for_updates() -> Result<Option<String>> {
    let current = crate::VERSION.to_string();
    let latest = tokio::task::spawn_blocking(fetch_latest)
        .await
        .context("join error")??;
    if is_newer(&latest, &current) {
        Ok(Some(latest))
    } else {
        Ok(None)
    }
}

fn fetch_latest() -> Result<String> {
    let resp = ureq::get(VERSION_URL)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .context("fetching VERSION")?;
    let txt = resp.into_string().context("reading VERSION body")?;
    Ok(txt.trim().to_string())
}

pub fn is_newer(remote: &str, current: &str) -> bool {
    let r = parse_semver(remote);
    let c = parse_semver(current);
    r > c
}

fn parse_semver(s: &str) -> (u32, u32, u32) {
    let mut it = s.trim().trim_start_matches('v').split('.');
    let a = it.next().unwrap_or("0").parse().unwrap_or(0);
    let b = it.next().unwrap_or("0").parse().unwrap_or(0);
    let c = it.next().unwrap_or("0").parse().unwrap_or(0);
    (a, b, c)
}

/// Download the release tarball for `version` and install both `lzsync`
/// and the bundled `lzsync-rsync` to `~/.cargo/bin/` and `~/.local/bin/`.
/// Returns the list of `lzsync` binary paths written (rsync paths omitted
/// for readability in the UI toast).
///
/// Safe to run while the caller is still executing the old binary: we write
/// to a `.new` temp file, remove the old inode, then rename. On Unix the
/// running process keeps its own inode open until exit.
pub async fn download_and_install(version: &str) -> Result<Vec<PathBuf>> {
    let ver = version.trim().trim_start_matches('v').to_string();
    let platform = detect_platform()?;
    let artifact = format!("lzsync-{platform}");
    let url =
        format!("https://github.com/FanFusion/lzsync/releases/download/v{ver}/{artifact}.tar.gz");
    let url_for_thread = url.clone();

    let bytes: Vec<u8> = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        let resp = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(180))
            .build()
            .get(&url_for_thread)
            .call()
            .with_context(|| format!("download {url_for_thread}"))?;
        if resp.status() != 200 {
            anyhow::bail!("HTTP {} from {}", resp.status(), url_for_thread);
        }
        let mut buf = Vec::new();
        resp.into_reader()
            .read_to_end(&mut buf)
            .context("read body")?;
        Ok(buf)
    })
    .await
    .context("join error")??;

    // Extract the two binaries we care about out of the gzipped tar. We use
    // tar via stdin instead of pulling in `tar` + `flate2` crates — every
    // Unix has tar, it keeps the binary slim, and the input is trusted
    // (signed by the GitHub Actions release).
    let extract_dir = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        let dir = std::env::temp_dir().join(format!("lzsync-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {dir:?}"))?;
        let mut child = std::process::Command::new("tar")
            .arg("-xzf")
            .arg("-")
            .arg("-C")
            .arg(&dir)
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("spawn tar")?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(&bytes).context("write tar stdin")?;
        }
        let out = child.wait_with_output().context("wait tar")?;
        if !out.status.success() {
            anyhow::bail!(
                "tar extract failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(dir)
    })
    .await
    .context("join error")??;

    let nested = extract_dir.join(&artifact);
    let src_lzsync = nested.join("lzsync");
    let src_rsync = nested.join("lzsync-rsync");
    if !src_lzsync.is_file() {
        anyhow::bail!("{src_lzsync:?} missing from release tarball");
    }

    let home = dirs::home_dir().context("HOME not set")?;
    let targets = [home.join(".cargo/bin"), home.join(".local/bin")];

    let mut installed = Vec::new();
    for dir in &targets {
        std::fs::create_dir_all(dir).with_context(|| format!("mkdir {dir:?}"))?;
        let dst_lzsync = dir.join("lzsync");
        atomic_install(&src_lzsync, &dst_lzsync)?;
        if src_rsync.is_file() {
            let dst_rsync = dir.join("lzsync-rsync");
            atomic_install(&src_rsync, &dst_rsync)?;
        }
        installed.push(dst_lzsync);
    }

    // Best-effort cleanup of the extracted tree so repeated updates don't
    // pile up temp dirs; ignore errors.
    let _ = std::fs::remove_dir_all(&extract_dir);

    if installed.is_empty() {
        anyhow::bail!("no install locations were writable");
    }
    Ok(installed)
}

/// Safe replace: write to `<dst>.new`, remove the old inode (so the
/// currently-running binary keeps its own file open), then rename.
fn atomic_install(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    let tmp = dst.with_extension("new");
    let payload = std::fs::read(src).with_context(|| format!("read {src:?}"))?;
    std::fs::write(&tmp, &payload).with_context(|| format!("write {tmp:?}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod {tmp:?}"))?;
    }
    let _ = std::fs::remove_file(dst);
    std::fs::rename(&tmp, dst).with_context(|| format!("rename -> {dst:?}"))?;
    Ok(())
}

fn detect_platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        ("macos", "aarch64") => Ok("macos-aarch64"),
        (os, arch) => anyhow::bail!("unsupported platform: {os}/{arch}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_patch() {
        assert!(is_newer("0.1.1", "0.1.0"));
    }

    #[test]
    fn newer_minor() {
        assert!(is_newer("0.2.0", "0.1.9"));
    }

    #[test]
    fn same_not_newer() {
        assert!(!is_newer("0.1.0", "0.1.0"));
    }

    #[test]
    fn older_not_newer() {
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn handles_v_prefix() {
        assert!(is_newer("v0.2.0", "0.1.0"));
    }

    #[test]
    fn detect_platform_is_one_of_known() {
        // Just make sure it returns a known string on the test host.
        let p = detect_platform().expect("supported test platform");
        assert!(
            matches!(
                p,
                "linux-x86_64" | "linux-aarch64" | "macos-x86_64" | "macos-aarch64"
            ),
            "got {p}"
        );
    }
}
