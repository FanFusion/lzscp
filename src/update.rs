use anyhow::{Context, Result};

const VERSION_URL: &str = "https://raw.githubusercontent.com/FanFusion/lzscp/main/VERSION";

/// Fetch remote VERSION. Returns Some(version) if newer than current, None if
/// same-or-older.
pub async fn check_for_updates() -> Result<Option<String>> {
    let current = crate::VERSION.to_string();
    let latest = tokio::task::spawn_blocking(|| fetch_latest())
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
}
