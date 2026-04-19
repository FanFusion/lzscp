use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::target::{ClipboardFormat, Group, SyncMode, Target};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub default_target: Option<String>,
    #[serde(default)]
    pub default_mode: SyncMode,
    #[serde(default)]
    pub clipboard_format: ClipboardFormat,
    #[serde(default = "default_clipboard_template")]
    pub clipboard_template: String,
    #[serde(default)]
    pub theme: String,
    #[serde(default, rename = "target")]
    pub targets: Vec<Target>,
    #[serde(default, rename = "group")]
    pub groups: Vec<Group>,
    #[serde(skip)]
    pub source: ConfigSource,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ConfigSource {
    #[default]
    Default,
    Project(PathBuf),
    Global(PathBuf),
}

fn default_clipboard_template() -> String {
    "{user}@{host}:{path}".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_target: None,
            default_mode: SyncMode::Auto,
            clipboard_format: ClipboardFormat::RemotePath,
            clipboard_template: default_clipboard_template(),
            theme: "mocha".to_string(),
            targets: vec![],
            groups: vec![],
            source: ConfigSource::Default,
        }
    }
}

impl Config {
    pub fn target_by_name(&self, name: &str) -> Option<&Target> {
        self.targets.iter().find(|t| t.name == name)
    }

    pub fn group_by_name(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.name == name)
    }

    #[allow(dead_code)]
    pub fn default_target(&self) -> Option<&Target> {
        self.default_target
            .as_ref()
            .and_then(|n| self.target_by_name(n))
            .or_else(|| self.targets.first())
    }
}

pub fn load() -> Result<Config> {
    let project = project_config_path();
    let global = global_config_path();

    let mut cfg = if let Some(p) = project
        && p.exists()
    {
        parse_from(&p, true)?
    } else if let Some(p) = global
        && p.exists()
    {
        parse_from(&p, false)?
    } else {
        Config::default()
    };

    // Pick a sensible default target if none set and we have any targets.
    if cfg.default_target.is_none()
        && let Some(first) = cfg.targets.first()
    {
        cfg.default_target = Some(first.name.clone());
    }
    Ok(cfg)
}

/// Build a Target from an SSH-config host, using a default remote inbox dir.
pub fn target_from_ssh_host(h: crate::ssh_config::SshHost) -> Target {
    Target {
        name: h.name.clone(),
        host: h.hostname.unwrap_or(h.name),
        user: h.user,
        remote_dir: "~/lzscp-inbox".to_string(),
        ssh_port: h.port,
        ssh_key: h.identity_file,
        clipboard_format: None,
    }
}

#[allow(dead_code)] // will be used by TUI edit flows in v0.2.0
pub fn save_global(cfg: &Config) -> Result<PathBuf> {
    let path = global_config_path().context("no global config dir")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {parent:?}"))?;
    }
    let text = toml::to_string_pretty(cfg).context("serialize config")?;
    std::fs::write(&path, text).with_context(|| format!("write {path:?}"))?;
    Ok(path)
}

pub fn parse_from(path: &Path, is_project: bool) -> Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config at {}", path.display()))?;
    let mut cfg: Config =
        toml::from_str(&raw).with_context(|| format!("parsing config at {}", path.display()))?;
    cfg.source = if is_project {
        ConfigSource::Project(path.to_path_buf())
    } else {
        ConfigSource::Global(path.to_path_buf())
    };
    validate(&cfg)?;
    Ok(cfg)
}

fn validate(cfg: &Config) -> Result<()> {
    use std::collections::HashSet;
    let mut names = HashSet::new();
    for t in &cfg.targets {
        anyhow::ensure!(!t.name.is_empty(), "target name must not be empty");
        anyhow::ensure!(!t.host.is_empty(), "target '{}' missing host", t.name);
        anyhow::ensure!(
            !t.remote_dir.is_empty(),
            "target '{}' missing remote_dir",
            t.name
        );
        anyhow::ensure!(
            names.insert(t.name.clone()),
            "duplicate target name: {}",
            t.name
        );
    }
    for g in &cfg.groups {
        anyhow::ensure!(!g.name.is_empty(), "group name must not be empty");
        for tn in &g.targets {
            anyhow::ensure!(
                cfg.targets.iter().any(|t| &t.name == tn),
                "group '{}' references unknown target '{}'",
                g.name,
                tn
            );
        }
        if let Some(primary) = &g.primary {
            anyhow::ensure!(
                g.targets.contains(primary),
                "group '{}' primary '{}' not in targets list",
                g.name,
                primary
            );
        }
    }
    if let Some(dt) = &cfg.default_target {
        anyhow::ensure!(
            cfg.target_by_name(dt).is_some() || cfg.group_by_name(dt).is_some(),
            "default_target '{}' not found",
            dt
        );
    }
    Ok(())
}

pub fn project_config_path() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".lzscp/config.toml"))
}

pub fn global_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lzscp/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_tmp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn parse_minimal() {
        let f = write_tmp(
            r#"
default_target = "dev"

[[target]]
name = "dev"
host = "dev.example.com"
user = "ubuntu"
remote_dir = "/home/ubuntu/uploads"
"#,
        );
        let cfg = parse_from(f.path(), true).unwrap();
        assert_eq!(cfg.default_target.as_deref(), Some("dev"));
        assert_eq!(cfg.targets.len(), 1);
        assert_eq!(cfg.targets[0].name, "dev");
        assert_eq!(cfg.targets[0].user.as_deref(), Some("ubuntu"));
    }

    #[test]
    fn parse_group_with_primary() {
        let f = write_tmp(
            r#"
[[target]]
name = "a"
host = "a.example.com"
remote_dir = "/tmp"

[[target]]
name = "b"
host = "b.example.com"
remote_dir = "/tmp"

[[group]]
name = "both"
targets = ["a", "b"]
primary = "a"
"#,
        );
        let cfg = parse_from(f.path(), false).unwrap();
        assert_eq!(cfg.groups.len(), 1);
        assert_eq!(cfg.groups[0].primary.as_deref(), Some("a"));
    }

    #[test]
    fn reject_duplicate_target_name() {
        let f = write_tmp(
            r#"
[[target]]
name = "dev"
host = "a"
remote_dir = "/tmp"

[[target]]
name = "dev"
host = "b"
remote_dir = "/tmp"
"#,
        );
        assert!(parse_from(f.path(), true).is_err());
    }

    #[test]
    fn reject_unknown_group_target() {
        let f = write_tmp(
            r#"
[[target]]
name = "a"
host = "a.example.com"
remote_dir = "/tmp"

[[group]]
name = "g"
targets = ["b"]
"#,
        );
        assert!(parse_from(f.path(), true).is_err());
    }

    #[test]
    fn reject_bad_default_target() {
        let f = write_tmp(
            r#"
default_target = "missing"

[[target]]
name = "a"
host = "a.example.com"
remote_dir = "/tmp"
"#,
        );
        assert!(parse_from(f.path(), true).is_err());
    }

    #[test]
    fn clipboard_format_per_target_override() {
        let f = write_tmp(
            r#"
clipboard_format = "remote_path"

[[target]]
name = "dev"
host = "dev.example.com"
user = "ubuntu"
remote_dir = "/tmp"
clipboard_format = "scp_style"
"#,
        );
        let cfg = parse_from(f.path(), true).unwrap();
        assert_eq!(cfg.clipboard_format, ClipboardFormat::RemotePath);
        assert_eq!(
            cfg.targets[0].clipboard_format,
            Some(ClipboardFormat::ScpStyle)
        );
    }
}

#[cfg(test)]
mod round_trip_tests {
    use super::*;
    use crate::target::Target;
    use tempfile::TempDir;

    #[test]
    fn targets_persist_across_save_and_reload() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        let mut cfg = Config {
            default_target: Some("devjp".into()),
            ..Config::default()
        };
        cfg.targets.push(Target {
            name: "devjp".into(),
            host: "1.2.3.4".into(),
            user: Some("ubuntu".into()),
            remote_dir: "~/lzscp-inbox".into(),
            ssh_port: Some(22),
            ssh_key: Some("~/.ssh/id_ed25519".into()),
            clipboard_format: None,
        });

        let text = toml::to_string_pretty(&cfg).unwrap();
        std::fs::write(&path, text).unwrap();

        let loaded = parse_from(&path, false).unwrap();
        assert_eq!(loaded.targets.len(), 1);
        assert_eq!(loaded.targets[0].name, "devjp");
        assert_eq!(loaded.targets[0].host, "1.2.3.4");
        assert_eq!(loaded.default_target.as_deref(), Some("devjp"));
    }
}
