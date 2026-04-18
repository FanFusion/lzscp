use std::path::PathBuf;

/// A resolved SSH config host entry for use as a transfer target.
///
/// We only surface hosts that have a concrete name (no `*` / `?` wildcards),
/// but we still honour `Host *` and other matching `Host <pattern>` blocks for
/// per-field fallback (e.g. a `Host *` setting `User ubuntu`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHost {
    pub name: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

pub fn load() -> Vec<SshHost> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let cfg_path = home.join(".ssh/config");
    if !cfg_path.exists() {
        return vec![];
    }
    let raw = match std::fs::read_to_string(&cfg_path) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    parse(&raw, Some(&cfg_path))
}

pub fn parse(raw: &str, base: Option<&std::path::Path>) -> Vec<SshHost> {
    let blocks = parse_blocks(raw, base, 0);

    // Separate wildcard / pattern blocks (used for defaults) from specific
    // host blocks (which produce Targets).
    let mut hosts: Vec<SshHost> = Vec::new();
    let mut default_candidates: Vec<&HostBlock> = Vec::new();
    for b in &blocks {
        for pat in &b.patterns {
            if is_wildcard(pat) || pat.contains('!') {
                default_candidates.push(b);
                break;
            }
        }
    }

    for block in &blocks {
        for pat in &block.patterns {
            if is_wildcard(pat) || pat.contains('!') {
                continue;
            }
            // Apply inheritance: later matching * / pattern blocks override as
            // fallback, SSH style (first set wins).
            let mut h = SshHost {
                name: pat.clone(),
                hostname: block.hostname.clone(),
                user: block.user.clone(),
                port: block.port,
                identity_file: block.identity_file.clone(),
            };
            for d in &default_candidates {
                if d.patterns.iter().any(|p| pattern_matches(p, pat)) {
                    h.hostname = h.hostname.or_else(|| d.hostname.clone());
                    h.user = h.user.or_else(|| d.user.clone());
                    h.port = h.port.or(d.port);
                    h.identity_file = h.identity_file.or_else(|| d.identity_file.clone());
                }
            }
            if h.hostname.is_none() {
                h.hostname = Some(pat.clone());
            }
            if !hosts.iter().any(|existing| existing.name == h.name) {
                hosts.push(h);
            }
        }
    }
    hosts
}

#[derive(Debug, Default)]
struct HostBlock {
    patterns: Vec<String>,
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
}

fn parse_blocks(raw: &str, base: Option<&std::path::Path>, depth: u8) -> Vec<HostBlock> {
    if depth > 4 {
        return vec![]; // guard against recursive Include loops
    }
    let mut blocks: Vec<HostBlock> = Vec::new();
    let mut current: Option<HostBlock> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (key, value) = match split_kv(trimmed) {
            Some(kv) => kv,
            None => continue,
        };
        let key_lower = key.to_ascii_lowercase();
        match key_lower.as_str() {
            "host" => {
                if let Some(prev) = current.take()
                    && !prev.patterns.is_empty()
                {
                    blocks.push(prev);
                }
                let patterns = value
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>();
                current = Some(HostBlock {
                    patterns,
                    ..Default::default()
                });
            }
            "match" => {
                // Skip Match blocks: they require runtime evaluation.
                if let Some(prev) = current.take()
                    && !prev.patterns.is_empty()
                {
                    blocks.push(prev);
                }
                current = None;
            }
            "include" => {
                // Expand Include lines relative to the parent config file.
                let paths = expand_include(value, base);
                for p in paths {
                    if let Ok(inner) = std::fs::read_to_string(&p) {
                        for b in parse_blocks(&inner, Some(&p), depth + 1) {
                            blocks.push(b);
                        }
                    }
                }
            }
            "hostname" => {
                if let Some(b) = current.as_mut() {
                    b.hostname = Some(value.to_string());
                }
            }
            "user" => {
                if let Some(b) = current.as_mut() {
                    b.user = Some(value.to_string());
                }
            }
            "port" => {
                if let Some(b) = current.as_mut()
                    && let Ok(n) = value.parse::<u16>()
                {
                    b.port = Some(n);
                }
            }
            "identityfile" => {
                if let Some(b) = current.as_mut() {
                    b.identity_file = Some(value.to_string());
                }
            }
            _ => {}
        }
    }
    if let Some(prev) = current.take()
        && !prev.patterns.is_empty()
    {
        blocks.push(prev);
    }
    blocks
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    // SSH config allows `Key = Value` or `Key Value`.
    let (k, rest) = line.split_once(|c: char| c.is_whitespace() || c == '=')?;
    let v = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
    Some((k, v.trim()))
}

fn is_wildcard(pat: &str) -> bool {
    pat.contains('*') || pat.contains('?')
}

fn pattern_matches(pattern: &str, target: &str) -> bool {
    // Minimal glob-ish matcher: support `*` and `?` only.
    let pbytes = pattern.as_bytes();
    let tbytes = target.as_bytes();
    glob_match(pbytes, 0, tbytes, 0)
}

fn glob_match(p: &[u8], pi: usize, t: &[u8], ti: usize) -> bool {
    if pi == p.len() {
        return ti == t.len();
    }
    match p[pi] {
        b'*' => {
            for ni in ti..=t.len() {
                if glob_match(p, pi + 1, t, ni) {
                    return true;
                }
            }
            false
        }
        b'?' => ti < t.len() && glob_match(p, pi + 1, t, ti + 1),
        c => ti < t.len() && t[ti] == c && glob_match(p, pi + 1, t, ti + 1),
    }
}

fn expand_include(value: &str, base: Option<&std::path::Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for tok in value.split_whitespace() {
        let expanded = shellexpand::tilde(tok).into_owned();
        let path = if std::path::Path::new(&expanded).is_absolute() {
            PathBuf::from(expanded)
        } else if let Some(b) = base {
            b.parent()
                .map(|d| d.join(&expanded))
                .unwrap_or_else(|| PathBuf::from(&expanded))
        } else {
            PathBuf::from(&expanded)
        };
        out.push(path);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_host() {
        let cfg = r#"
Host devsg
    HostName 1.2.3.4
    User ubuntu
    Port 22
    IdentityFile ~/.ssh/dev_sg.pem
"#;
        let hosts = parse(cfg, None);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "devsg");
        assert_eq!(hosts[0].hostname.as_deref(), Some("1.2.3.4"));
        assert_eq!(hosts[0].user.as_deref(), Some("ubuntu"));
        assert_eq!(hosts[0].port, Some(22));
    }

    #[test]
    fn wildcards_only_used_as_defaults() {
        let cfg = r#"
Host *
    User ubuntu

Host devsg
    HostName 1.2.3.4
"#;
        let hosts = parse(cfg, None);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].user.as_deref(), Some("ubuntu"));
    }

    #[test]
    fn explicit_overrides_wildcard() {
        let cfg = r#"
Host *
    User ubuntu

Host bastion
    HostName bastion.example.com
    User admin
"#;
        let hosts = parse(cfg, None);
        assert_eq!(hosts[0].user.as_deref(), Some("admin"));
    }

    #[test]
    fn multiple_host_patterns_expand() {
        let cfg = r#"
Host a b c
    HostName shared.example.com
"#;
        let hosts = parse(cfg, None);
        let names: Vec<_> = hosts.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        for h in &hosts {
            assert_eq!(h.hostname.as_deref(), Some("shared.example.com"));
        }
    }

    #[test]
    fn comments_and_blank_lines_ignored() {
        let cfg = r#"
# this is a comment

Host dev
    # another comment
    HostName dev.example.com
"#;
        let hosts = parse(cfg, None);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "dev");
    }

    #[test]
    fn key_equals_value_form() {
        let cfg = r#"
Host dev
    HostName=dev.example.com
    User=ubuntu
"#;
        let hosts = parse(cfg, None);
        assert_eq!(hosts[0].hostname.as_deref(), Some("dev.example.com"));
        assert_eq!(hosts[0].user.as_deref(), Some("ubuntu"));
    }

    #[test]
    fn falls_back_hostname_to_name() {
        let cfg = r#"
Host onlyname
    User root
"#;
        let hosts = parse(cfg, None);
        assert_eq!(hosts[0].hostname.as_deref(), Some("onlyname"));
    }
}
