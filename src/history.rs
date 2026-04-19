use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One transfer of one local file to one remote host. Records are keyed by
/// (local_path, target_name) — re-syncing the same file to the same host
/// updates the in-memory entry and appends a fresh line to the per-host
/// JSONL so readers of the file see the latest state on load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub local_path: String,
    pub local_name: String,
    pub size: u64,
    pub target_name: String,
    pub target_host: String,
    pub remote_path: String,
    pub synced_at: u64, // unix epoch seconds
    pub status: HistoryStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    Completed,
    Failed,
}

pub struct HistoryStore {
    pub entries: Vec<HistoryEntry>,
    pub base: PathBuf,
}

impl Default for HistoryStore {
    fn default() -> Self {
        Self {
            entries: vec![],
            base: history_dir().unwrap_or_else(|| PathBuf::from(".lzscp/history")),
        }
    }
}

impl HistoryStore {
    /// Load all per-host JSONL files in `base_dir/history/`. Newer records
    /// for the same (local_path, target_name) supersede older ones. Returns
    /// an empty store if the directory doesn't exist yet.
    pub fn load() -> Result<Self> {
        let base = history_dir().context("no config dir")?;
        Self::load_from(&base)
    }

    pub fn load_from(base: &Path) -> Result<Self> {
        let mut latest: HashMap<(String, String), HistoryEntry> = HashMap::new();
        if base.exists() {
            for entry in std::fs::read_dir(base).with_context(|| format!("readdir {base:?}"))? {
                let entry = entry?;
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                    continue;
                }
                let f = match std::fs::File::open(&p) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                for line in BufReader::new(f).lines().map_while(Result::ok) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let e: HistoryEntry = match serde_json::from_str(&line) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    let key = (e.local_path.clone(), e.target_name.clone());
                    latest
                        .entry(key)
                        .and_modify(|cur| {
                            if e.synced_at >= cur.synced_at {
                                *cur = e.clone();
                            }
                        })
                        .or_insert(e);
                }
            }
        }
        let mut entries: Vec<HistoryEntry> = latest.into_values().collect();
        // Most recent first.
        entries.sort_by(|a, b| b.synced_at.cmp(&a.synced_at));
        Ok(Self {
            entries,
            base: base.to_path_buf(),
        })
    }

    pub fn append(&mut self, entry: HistoryEntry) -> Result<()> {
        std::fs::create_dir_all(&self.base).with_context(|| format!("mkdir {:?}", self.base))?;
        let path = self
            .base
            .join(format!("{}.jsonl", sanitize_filename(&entry.target_name)));
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {path:?}"))?;
        let line = serde_json::to_string(&entry).context("serialize history entry")?;
        writeln!(f, "{line}").with_context(|| format!("write {path:?}"))?;

        // Replace or prepend in memory.
        let key = (entry.local_path.clone(), entry.target_name.clone());
        if let Some(pos) = self
            .entries
            .iter()
            .position(|e| (e.local_path.clone(), e.target_name.clone()) == key)
        {
            self.entries[pos] = entry;
            let updated = self.entries.remove(pos);
            self.entries.insert(0, updated);
        } else {
            self.entries.insert(0, entry);
        }
        Ok(())
    }

    /// Case-insensitive substring filter over local_name, local_path, and
    /// target_name. Empty query returns everything.
    pub fn filter(&self, query: &str) -> Vec<&HistoryEntry> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.entries.iter().collect();
        }
        self.entries
            .iter()
            .filter(|e| {
                e.local_name.to_lowercase().contains(&q)
                    || e.local_path.to_lowercase().contains(&q)
                    || e.target_name.to_lowercase().contains(&q)
            })
            .collect()
    }
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn format_time_ago(epoch: u64) -> String {
    let now = now_epoch();
    if now < epoch {
        return "just now".to_string();
    }
    let d = now - epoch;
    if d < 60 {
        return "just now".to_string();
    }
    if d < 3600 {
        return format!("{}m ago", d / 60);
    }
    if d < 86_400 {
        return format!("{}h ago", d / 3600);
    }
    if d < 604_800 {
        return format!("{}d ago", d / 86_400);
    }
    format!("{}w ago", d / 604_800)
}

pub fn history_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lzscp/history"))
}

fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "unknown".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(local: &str, target: &str, remote: &str, at: u64) -> HistoryEntry {
        HistoryEntry {
            local_path: local.to_string(),
            local_name: Path::new(local)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            size: 123,
            target_name: target.to_string(),
            target_host: format!("user@{target}"),
            remote_path: remote.to_string(),
            synced_at: at,
            status: HistoryStatus::Completed,
        }
    }

    #[test]
    fn append_then_reload_round_trip() {
        let tmp = TempDir::new().unwrap();
        let mut store = HistoryStore::load_from(tmp.path()).unwrap();
        assert!(store.entries.is_empty());

        store
            .append(entry("/a/b/shot.png", "devjp", "/r/shot.png", 100))
            .unwrap();
        store
            .append(entry("/a/b/report.pdf", "dev", "/r/report.pdf", 200))
            .unwrap();

        let reloaded = HistoryStore::load_from(tmp.path()).unwrap();
        assert_eq!(reloaded.entries.len(), 2);
        assert_eq!(reloaded.entries[0].local_name, "report.pdf"); // newest first
        assert_eq!(reloaded.entries[1].local_name, "shot.png");
    }

    #[test]
    fn dedupe_on_reload_keeps_latest() {
        let tmp = TempDir::new().unwrap();
        let mut store = HistoryStore::load_from(tmp.path()).unwrap();
        store
            .append(entry("/a/shot.png", "devjp", "/r/old.png", 100))
            .unwrap();
        store
            .append(entry("/a/shot.png", "devjp", "/r/new.png", 300))
            .unwrap();
        let reloaded = HistoryStore::load_from(tmp.path()).unwrap();
        assert_eq!(reloaded.entries.len(), 1);
        assert_eq!(reloaded.entries[0].remote_path, "/r/new.png");
    }

    #[test]
    fn same_file_different_targets_are_separate_entries() {
        let tmp = TempDir::new().unwrap();
        let mut store = HistoryStore::load_from(tmp.path()).unwrap();
        store
            .append(entry("/a/shot.png", "devjp", "/r/jp/shot.png", 100))
            .unwrap();
        store
            .append(entry("/a/shot.png", "dev", "/r/dev/shot.png", 110))
            .unwrap();
        let reloaded = HistoryStore::load_from(tmp.path()).unwrap();
        assert_eq!(reloaded.entries.len(), 2);
    }

    #[test]
    fn filter_matches_name_path_or_target() {
        let tmp = TempDir::new().unwrap();
        let mut store = HistoryStore::load_from(tmp.path()).unwrap();
        store
            .append(entry("/a/shot.png", "devjp", "/r/shot.png", 100))
            .unwrap();
        store
            .append(entry("/b/report.pdf", "dev", "/r/report.pdf", 200))
            .unwrap();
        assert_eq!(store.filter("shot").len(), 1);
        assert_eq!(store.filter("devjp").len(), 1);
        assert_eq!(store.filter("/b/").len(), 1);
        assert_eq!(store.filter("xyz").len(), 0);
        assert_eq!(store.filter("").len(), 2);
    }

    #[test]
    fn sanitize_filename_replaces_slashes() {
        assert_eq!(sanitize_filename("dev/jp"), "dev_jp");
        assert_eq!(sanitize_filename("a@b"), "a_b");
        assert_eq!(sanitize_filename("ok-name_1"), "ok-name_1");
    }

    #[test]
    fn format_time_ago_buckets() {
        let now = now_epoch();
        assert_eq!(format_time_ago(now), "just now");
        assert_eq!(format_time_ago(now.saturating_sub(120)), "2m ago");
        assert_eq!(format_time_ago(now.saturating_sub(3 * 3600)), "3h ago");
        assert_eq!(format_time_ago(now.saturating_sub(2 * 86_400)), "2d ago");
    }
}
