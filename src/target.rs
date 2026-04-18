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
