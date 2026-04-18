use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};

const VERSION_URL: &str = "https://raw.githubusercontent.com/FanFusion/lzscp/main/VERSION";

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

/// Download the release binary for `version` and install to both
/// `~/.cargo/bin/lzscp` and `~/.local/bin/lzscp`. Returns the list of paths
/// that were actually written.
///
/// Safe to run while the caller is still executing the old binary: we write
/// to a `.new` temp file, remove the old inode, then rename. On Unix the
/// running process keeps its own inode open until exit.
pub async fn download_and_install(version: &str) -> Result<Vec<PathBuf>> {
    let ver = version.trim().trim_start_matches('v').to_string();
    let platform = detect_platform()?;
    let url =
        format!("https://github.com/FanFusion/lzscp/releases/download/v{ver}/lzscp-{platform}");
    let url_for_thread = url.clone();

    let bytes: Vec<u8> = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        let resp = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(120))
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

    let home = dirs::home_dir().context("HOME not set")?;
    let targets = [home.join(".cargo/bin/lzscp"), home.join(".local/bin/lzscp")];

    let mut installed = Vec::new();
    for dst in &targets {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("mkdir {parent:?}"))?;
        }
        let tmp = dst.with_extension("new");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {tmp:?}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
                .with_context(|| format!("chmod {tmp:?}"))?;
        }
        // Remove the old file first — on Linux you can't overwrite a running
        // binary ("text file busy"), but rename-over works because the kernel
        // keeps the running inode alive.
        let _ = std::fs::remove_file(dst);
        std::fs::rename(&tmp, dst).with_context(|| format!("rename -> {dst:?}"))?;
        installed.push(dst.clone());
    }

    if installed.is_empty() {
        anyhow::bail!("no install locations were writable");
    }
    Ok(installed)
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
