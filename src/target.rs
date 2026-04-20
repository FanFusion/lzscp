use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Target {
    pub name: String,
    pub host: String,
    #[serde(default)]
    pub user: Option<String>,
    pub remote_dir: String,
    #[serde(default)]
    pub ssh_port: Option<u16>,
    #[serde(default)]
    pub ssh_key: Option<String>,
    #[serde(default)]
    pub clipboard_format: Option<ClipboardFormat>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Group {
    pub name: String,
    pub targets: Vec<String>,
    #[serde(default)]
    pub primary: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardFormat {
    #[default]
    RemotePath,
    ScpStyle,
    SshPath,
    Custom,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    #[default]
    Auto,
    Manual,
}

/// One monitored directory. Files matching `patterns` that land in `path` are
/// auto-synced to every name in `targets`. Multi-instance safety is enforced
/// via a per-folder lock; see `crate::watch::LockHandle`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchConfig {
    pub name: String,
    pub path: String,
    pub targets: Vec<String>,
    #[serde(default = "default_watch_patterns")]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub catchup: CatchupMode,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default)]
    pub recursive: bool,
    /// How to handle a remote file that already exists with the same name.
    /// Watch defaults to `rename` (auto-suffix) so a screenshot workflow
    /// doesn't stall on a modal for every repeat name; set to `prompt` to
    /// get the interactive confirm behavior instead.
    #[serde(default = "default_watch_on_conflict")]
    pub on_conflict: ConflictAction,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictAction {
    /// Pop a modal; user picks o/s/r/Esc.
    #[default]
    Prompt,
    /// Skip the ssh preflight and let rsync overwrite silently.
    Overwrite,
    /// Auto-suffix the new file with `-N` so both versions survive.
    Rename,
    /// Do not transfer; record a `skipped` outcome.
    Skip,
}

fn default_watch_on_conflict() -> ConflictAction {
    ConflictAction::Rename
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CatchupMode {
    /// Show an "N new since X" badge; the user presses `r` to sync the batch.
    #[default]
    Prompt,
    /// Sync every file whose mtime is newer than the last recorded `last_seen`.
    Auto,
    /// Treat everything already on disk as baseline; never auto-sync the past.
    Ignore,
}

fn default_watch_patterns() -> Vec<String> {
    vec![
        "*.png".into(),
        "*.jpg".into(),
        "*.jpeg".into(),
        "*.heic".into(),
        "*.webp".into(),
        "*.gif".into(),
    ]
}

fn default_debounce_ms() -> u64 {
    500
}

/// One port-forward tunnel (ssh `-L`). Refers to an existing `[[target]]` by
/// name so we reuse all the SSH connection details (host, user, key, port)
/// rather than repeating them. Multi-instance safety for a given local port
/// is enforced via `crate::tunnel::LockHandle`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TunnelConfig {
    pub name: String,
    pub target: String,
    pub local_port: u16,
    #[serde(default = "default_tunnel_remote_host")]
    pub remote_host: String,
    pub remote_port: u16,
    #[serde(default = "default_tunnel_bind_address")]
    pub bind_address: String,
    #[serde(default)]
    pub autostart: bool,
}

fn default_tunnel_remote_host() -> String {
    "localhost".into()
}

fn default_tunnel_bind_address() -> String {
    "127.0.0.1".into()
}

impl Target {
    #[allow(dead_code)]
    pub fn user_str(&self) -> &str {
        self.user.as_deref().unwrap_or("")
    }

    pub fn display_endpoint(&self) -> String {
        if let Some(user) = &self.user {
            format!("{user}@{}:{}", self.host, self.remote_dir)
        } else {
            format!("{}:{}", self.host, self.remote_dir)
        }
    }

    pub fn ssh_port(&self) -> u16 {
        self.ssh_port.unwrap_or(22)
    }
}
