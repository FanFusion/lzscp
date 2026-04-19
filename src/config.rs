use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::target::{ClipboardFormat, Group, SyncMode, Target, WatchConfig};

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
    #[serde(default, rename = "watch")]
    pub watches: Vec<WatchConfig>,
    #[serde(skip)]
    pub source: ConfigSource,
    /// Non-persisted notice shown to the user via a toast on first launch
    /// after upgrading from lzscp to lzsync.
    #[serde(skip)]
    pub migration_notice: Option<String>,
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
            watches: vec![],
            source: ConfigSource::Default,
            migration_notice: None,
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
    // Must run before the global read below — if the user is upgrading from
    // lzscp the new global dir doesn't exist yet.
    let notice = migrate_legacy_config().unwrap_or_else(|e| Some(format!("migration failed: {e}")));

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
    cfg.migration_notice = notice;
    Ok(cfg)
}

/// Build a Target from an SSH-config host, using a default remote inbox dir.
pub fn target_from_ssh_host(h: crate::ssh_config::SshHost) -> Target {
    Target {
        name: h.name.clone(),
        host: h.hostname.unwrap_or(h.name),
        user: h.user,
        remote_dir: "~/lzsync-inbox".to_string(),
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
    let mut watch_names = HashSet::new();
    for w in &cfg.watches {
        anyhow::ensure!(!w.name.is_empty(), "watch name must not be empty");
        anyhow::ensure!(!w.path.is_empty(), "watch '{}' missing path", w.name);
        anyhow::ensure!(
            !w.targets.is_empty(),
            "watch '{}' must reference at least one target",
            w.name
        );
        anyhow::ensure!(
            watch_names.insert(w.name.clone()),
            "duplicate watch name: {}",
            w.name
        );
        for tn in &w.targets {
            anyhow::ensure!(
                cfg.targets.iter().any(|t| &t.name == tn) || cfg.group_by_name(tn).is_some(),
                "watch '{}' references unknown target '{}'",
                w.name,
                tn
            );
        }
    }
    Ok(())
}

pub fn project_config_path() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".lzsync/config.toml"))
}

pub fn global_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lzsync/config.toml"))
}

/// Pre-0.4.0 global config lived under `~/.config/lzscp/` (when the binary was
/// named lzscp). Returned so we can migrate legacy configs on startup.
pub fn legacy_global_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lzscp"))
}

pub fn current_global_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lzsync"))
}

/// If the user is upgrading from lzscp (<0.4.0), their config and history live
/// under `~/.config/lzscp/`. On first launch under lzsync we recursively copy
/// that directory into `~/.config/lzsync/` so they don't lose their targets or
/// transfer history. The old directory is left in place as a backup. The copy
/// is a no-op when the new directory already exists or the old one doesn't.
/// Returns Ok(Some(message)) on a successful migration, Ok(None) if nothing to
/// do.
pub fn migrate_legacy_config() -> Result<Option<String>> {
    let (Some(new_dir), Some(old_dir)) = (current_global_dir(), legacy_global_dir()) else {
        return Ok(None);
    };
    if new_dir.exists() || !old_dir.exists() {
        return Ok(None);
    }
    copy_dir_all(&old_dir, &new_dir)
        .with_context(|| format!("migrate {old_dir:?} -> {new_dir:?}"))?;
    Ok(Some(format!(
        "migrated config from {} (backup kept)",
        old_dir.display()
    )))
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if file_type.is_file() {
            std::fs::copy(&from, &to)?;
        }
        // Symlinks are skipped; lzsync config shouldn't contain any.
    }
    Ok(())
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
mod migration_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn copy_dir_all_recursively_copies_nested_files() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("config.toml"), "hello").unwrap();
        std::fs::write(src.join("sub/history.jsonl"), "world").unwrap();

        copy_dir_all(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("config.toml")).unwrap(),
            "hello"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("sub/history.jsonl")).unwrap(),
            "world"
        );
        // Source preserved.
        assert!(src.join("config.toml").exists());
    }

    #[test]
    fn copy_dir_all_errors_on_missing_source() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("missing");
        let dst = tmp.path().join("dst");
        assert!(copy_dir_all(&src, &dst).is_err());
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
            remote_dir: "~/lzsync-inbox".into(),
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
