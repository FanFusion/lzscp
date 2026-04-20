//! Folder watching: per-folder lock, polling engine, catchup on startup.
//!
//! A watch is scoped to a directory path. At most one process may hold the
//! lock for a given path at any time; this prevents two lzsync instances
//! watching the same folder from both uploading the same screenshot twice.
//!
//! The lock file is a small JSON blob under
//! `~/.config/lzsync/watch-locks/<fnv1a(abs_path)>.lock` containing the pid
//! and started_at of the owning process. On acquisition we atomically create
//! the file (`O_EXCL`) — if it already exists, we peek at the pid and, if the
//! process is dead, take it over. This handles the crashed-lzsync case
//! without requiring any daemon or external cleanup.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::target::WatchConfig;

#[derive(Debug, Serialize, Deserialize)]
struct LockRecord {
    pid: u32,
    started_at: u64,
    /// Absolute path being watched — recorded for human diagnosis when a lock
    /// file is inspected directly. Not used for logic.
    path: String,
}

/// Owned lock on a folder. Dropping the handle removes the lock file (or
/// leaves it if it's been overwritten by another process since acquisition).
#[derive(Debug)]
pub struct LockHandle {
    #[allow(dead_code)] // retained for logging / diagnostics
    pub watch_name: String,
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

/// Try to take the folder lock. On success returns a handle that releases
/// the lock when dropped. On `LockError::Held` the caller can display the
/// offending PID to the user.
#[allow(dead_code)] // wired up by app.rs in the UI stage
pub fn try_acquire_lock(
    watch_name: &str,
    abs_path: &Path,
) -> std::result::Result<LockHandle, LockError> {
    try_acquire_lock_in(watch_name, abs_path, &default_lock_dir())
}

fn try_acquire_lock_in(
    watch_name: &str,
    abs_path: &Path,
    lock_dir: &Path,
) -> std::result::Result<LockHandle, LockError> {
    fs::create_dir_all(lock_dir).map_err(|e| LockError::Io(format!("mkdir {lock_dir:?}: {e}")))?;
    let lock_path = lock_dir.join(format!(
        "{}.lock",
        fnv1a_hex(abs_path.to_string_lossy().as_ref())
    ));
    let pid = std::process::id();
    let record = LockRecord {
        pid,
        started_at: now_epoch(),
        path: abs_path.to_string_lossy().into_owned(),
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
                watch_name: watch_name.to_string(),
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
                        watch_name: watch_name.to_string(),
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
                watch_name: watch_name.to_string(),
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
        .map(|d| d.join("lzsync/watch-locks"))
        .unwrap_or_else(|| PathBuf::from(".lzsync/watch-locks"))
}

// =============================================================================
// Persistent watch state (per-watch last_seen_mtime for catchup)
// =============================================================================

/// Persists, per watch name, the last time the user's lzsync exited cleanly
/// (or last catchup ran). Used at startup to decide which files to treat as
/// "catchup candidates". Format: `~/.config/lzsync/watch-state.json`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PersistedState {
    #[serde(default)]
    pub watches: std::collections::HashMap<String, PersistedWatchState>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PersistedWatchState {
    #[serde(default)]
    pub last_seen_mtime: u64,
    #[serde(default)]
    pub last_exit_at: u64,
}

pub fn state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lzsync/watch-state.json"))
}

pub fn load_state() -> PersistedState {
    let Some(path) = state_path() else {
        return PersistedState::default();
    };
    let Ok(txt) = std::fs::read_to_string(&path) else {
        return PersistedState::default();
    };
    serde_json::from_str(&txt).unwrap_or_default()
}

pub fn save_state(state: &PersistedState) -> std::io::Result<()> {
    let Some(path) = state_path() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no config dir",
        ));
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let txt = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::write(&path, txt)
}

// =============================================================================
// Polling engine
// =============================================================================

/// Event emitted by a running watch task. Consumers observe these via the
/// `tx` channel passed to `start_poller`.
#[derive(Debug, Clone)]
#[allow(dead_code)] // variants other than NewFile are used in later stages
pub enum WatchEvent {
    /// A file matched `patterns`, has been on disk at least `debounce_ms`,
    /// and its size hasn't changed since we last sampled it. Safe to sync.
    NewFile {
        watch_name: String,
        path: PathBuf,
        size: u64,
    },
    /// On startup we observed N files whose mtime is newer than the last
    /// recorded `last_seen_mtime`. Only emitted for watches with
    /// `catchup = "prompt"` — in Auto mode we emit individual NewFile events
    /// instead; in Ignore we emit nothing.
    CatchupDetected {
        watch_name: String,
        paths: Vec<PathBuf>,
    },
    /// The poller's own lock file has disappeared or been rewritten by
    /// another process. The poller has stopped; the UI should show this
    /// watch as unlocked.
    LockLost { watch_name: String, reason: String },
    /// The poller exited cleanly (user toggled the watch off, or app is
    /// shutting down).
    Stopped { watch_name: String },
}

/// Handle returned by `start_poller`. Dropping or calling `stop()` signals
/// the background task to exit on its next tick, which releases the
/// underlying filesystem lock.
#[derive(Debug)]
#[allow(dead_code)] // consumed by app.rs in the UI stage
pub struct WatchHandle {
    pub watch_name: String,
    stop: Arc<AtomicBool>,
}

impl WatchHandle {
    #[allow(dead_code)]
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Start a background task that polls `config.path` once per second and
/// emits `WatchEvent::NewFile` whenever a file matching `patterns` has been
/// stable on disk for at least `config.debounce_ms`.
///
/// The task owns `lock`, so dropping the returned `WatchHandle` releases
/// the filesystem lock along with shutting down the poller.
///
/// `last_seen_mtime` controls what happens to files that already exist in
/// the folder when the poller starts (catchup):
/// - `CatchupMode::Ignore`: everything is baseline (nothing re-sent)
/// - `CatchupMode::Auto`: files with mtime > last_seen_mtime are re-sent
/// - `CatchupMode::Prompt`: files with mtime > last_seen_mtime are reported
///   via `WatchEvent::CatchupDetected`; the app decides when to emit.
#[allow(dead_code)] // wired up by app.rs in the UI stage
pub fn start_poller(
    config: WatchConfig,
    lock: LockHandle,
    tx: UnboundedSender<WatchEvent>,
    last_seen_mtime: u64,
) -> WatchHandle {
    start_poller_with_interval(config, lock, tx, last_seen_mtime, DEFAULT_POLL_INTERVAL)
}

fn start_poller_with_interval(
    config: WatchConfig,
    lock: LockHandle,
    tx: UnboundedSender<WatchEvent>,
    last_seen_mtime: u64,
    poll_interval: Duration,
) -> WatchHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);
    let watch_name = config.name.clone();
    tokio::spawn(async move {
        let _owned_lock = lock; // dropped when this task exits
        poller_loop(config, tx, stop_clone, poll_interval, last_seen_mtime).await;
    });
    WatchHandle { watch_name, stop }
}

async fn poller_loop(
    config: WatchConfig,
    tx: UnboundedSender<WatchEvent>,
    stop: Arc<AtomicBool>,
    poll_interval: Duration,
    last_seen_mtime: u64,
) {
    let watch_root = match resolve_path(&config.path) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(WatchEvent::LockLost {
                watch_name: config.name.clone(),
                reason: format!("resolve path: {e}"),
            });
            return;
        }
    };
    let debounce = Duration::from_millis(config.debounce_ms);

    // Initial catchup: partition files already present into "newer than
    // last_seen_mtime" (candidates) and "older" (pure baseline). Behaviour
    // depends on the watch's catchup mode.
    let (catchup_candidates, baseline_files) = partition_catchup(
        &watch_root,
        &config.patterns,
        config.recursive,
        last_seen_mtime,
    );
    let mut known: HashSet<PathBuf> = baseline_files.into_iter().collect();

    match config.catchup {
        crate::target::CatchupMode::Ignore => {
            // Treat all existing files as baseline; emit nothing.
            for p in catchup_candidates {
                known.insert(p);
            }
        }
        crate::target::CatchupMode::Auto => {
            // Auto-emit each candidate as NewFile; they go to baseline.
            for (p, size) in get_sizes(&catchup_candidates) {
                let _ = tx.send(WatchEvent::NewFile {
                    watch_name: config.name.clone(),
                    path: p.clone(),
                    size,
                });
                known.insert(p);
            }
        }
        crate::target::CatchupMode::Prompt => {
            // Tell the app; it'll decide when/if to flush. Until the user
            // hits `r`, candidates stay out of `known` so the UI's "N new"
            // counter is meaningful — but we also don't want to re-report
            // the same file on every tick, so we pre-mark them as known here.
            if !catchup_candidates.is_empty() {
                let _ = tx.send(WatchEvent::CatchupDetected {
                    watch_name: config.name.clone(),
                    paths: catchup_candidates.clone(),
                });
            }
            for p in catchup_candidates {
                known.insert(p);
            }
        }
    }
    let mut pending: HashMap<PathBuf, (Instant, u64)> = HashMap::new();
    let mut ticker = tokio::time::interval(poll_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let current: Vec<(PathBuf, u64)> =
            list_files(&watch_root, &config.patterns, config.recursive);
        let current_set: HashSet<&PathBuf> = current.iter().map(|(p, _)| p).collect();

        // Clean pending for files that vanished before we could emit.
        pending.retain(|p, _| current_set.contains(p));

        let now = Instant::now();
        for (file_path, size) in &current {
            if known.contains(file_path) {
                continue;
            }
            match pending.get(file_path).copied() {
                None => {
                    pending.insert(file_path.clone(), (now, *size));
                }
                Some((last_changed, last_size)) => {
                    if *size != last_size {
                        // Still growing; reset the timer.
                        pending.insert(file_path.clone(), (now, *size));
                    } else if now.duration_since(last_changed) >= debounce {
                        let _ = tx.send(WatchEvent::NewFile {
                            watch_name: config.name.clone(),
                            path: file_path.clone(),
                            size: *size,
                        });
                        known.insert(file_path.clone());
                        pending.remove(file_path);
                    }
                }
            }
        }
    }
    let _ = tx.send(WatchEvent::Stopped {
        watch_name: config.name,
    });
}

/// Expand `~` / env vars and attempt to canonicalize. Falls back to the
/// expanded (but possibly relative) path if canonicalize fails so a
/// non-existent dir still yields a usable error trail.
pub fn resolve_path(raw: &str) -> std::io::Result<PathBuf> {
    let expanded = shellexpand::full(raw)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
    let p = PathBuf::from(expanded.into_owned());
    match fs::canonicalize(&p) {
        Ok(abs) => Ok(abs),
        Err(_) => Ok(p),
    }
}

fn list_files(root: &Path, patterns: &[String], recursive: bool) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    walk_dir(root, patterns, recursive, &mut out);
    out
}

/// Scan `root` and split matching files into (newer-than-threshold, older).
/// Files whose mtime can't be read are treated as old (safe — baseline them).
fn partition_catchup(
    root: &Path,
    patterns: &[String],
    recursive: bool,
    threshold_epoch: u64,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut newer = Vec::new();
    let mut older = Vec::new();
    for (path, _size) in list_files(root, patterns, recursive) {
        let mtime = fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if mtime > threshold_epoch {
            newer.push(path);
        } else {
            older.push(path);
        }
    }
    (newer, older)
}

fn get_sizes(paths: &[PathBuf]) -> Vec<(PathBuf, u64)> {
    paths
        .iter()
        .map(|p| (p.clone(), fs::metadata(p).map(|m| m.len()).unwrap_or(0)))
        .collect()
}

fn walk_dir(dir: &Path, patterns: &[String], recursive: bool, out: &mut Vec<(PathBuf, u64)>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if ft.is_dir() {
            if recursive {
                walk_dir(&path, patterns, recursive, out);
            }
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !matches_any(patterns, name) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push((path, size));
    }
}

/// Case-insensitive glob match against a file's basename. Supports `*` (any
/// sequence) and `?` (single char). An empty patterns list matches everything.
pub fn matches_any(patterns: &[String], filename: &str) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let name_lc = filename.to_lowercase();
    patterns
        .iter()
        .any(|p| glob_match(&p.to_lowercase(), &name_lc))
}

fn glob_match(pattern: &str, s: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = s.chars().collect();
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while si < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stable, filesystem-safe hash of an absolute path. Used purely to derive a
/// lock filename — no cryptographic guarantees needed. FNV-1a 64-bit:
/// deterministic across processes and platforms, 5 lines, no dependency.
fn fnv1a_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // kill(pid, 0) sends no signal but performs the permission/existence
    // check. Returns 0 if the process exists; -1 + ESRCH if it doesn't.
    // EPERM ("process exists, not allowed to signal it") also counts as alive.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    let err = std::io::Error::last_os_error();
    matches!(err.raw_os_error(), Some(libc::EPERM))
}

#[cfg(not(unix))]
fn is_pid_alive(_pid: u32) -> bool {
    // On non-Unix we can't cheaply probe. Treat as alive to be safe; the
    // user can manually remove the lock file if needed.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn atomic_create_returns_handle() {
        let tmp = TempDir::new().unwrap();
        let watch_path = tmp.path().join("target");
        let lock_dir = tmp.path().join("locks");
        let h = try_acquire_lock_in("w1", &watch_path, &lock_dir).unwrap();
        assert_eq!(h.owner_pid, std::process::id());
        assert!(h.lock_path.exists());
    }

    #[test]
    fn second_acquire_returns_held_when_alive() {
        let tmp = TempDir::new().unwrap();
        let watch_path = tmp.path().join("target");
        let lock_dir = tmp.path().join("locks");
        let _first = try_acquire_lock_in("w1", &watch_path, &lock_dir).unwrap();
        // Overwrite the lock's pid field to our own live pid to simulate a
        // different live process (since is_pid_alive checks actual liveness
        // and our test process is obviously live).
        match try_acquire_lock_in("w1", &watch_path, &lock_dir) {
            Err(LockError::Held { pid }) => assert_eq!(pid, std::process::id()),
            other => panic!("expected Held, got {other:?}"),
        }
    }

    #[test]
    fn stale_lock_is_reclaimed() {
        let tmp = TempDir::new().unwrap();
        let watch_path = tmp.path().join("target");
        let lock_dir = tmp.path().join("locks");
        fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join(format!(
            "{}.lock",
            fnv1a_hex(watch_path.to_string_lossy().as_ref())
        ));
        // Plant a lock belonging to a pid that's almost certainly dead.
        let stale = LockRecord {
            pid: 999_999_999,
            started_at: 0,
            path: watch_path.to_string_lossy().into_owned(),
        };
        fs::write(&lock_path, serde_json::to_string(&stale).unwrap()).unwrap();

        let h = try_acquire_lock_in("w1", &watch_path, &lock_dir).unwrap();
        assert_eq!(h.owner_pid, std::process::id());
    }

    #[test]
    fn corrupt_lock_is_overwritten() {
        let tmp = TempDir::new().unwrap();
        let watch_path = tmp.path().join("target");
        let lock_dir = tmp.path().join("locks");
        fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join(format!(
            "{}.lock",
            fnv1a_hex(watch_path.to_string_lossy().as_ref())
        ));
        fs::write(&lock_path, "not json at all").unwrap();
        let h = try_acquire_lock_in("w1", &watch_path, &lock_dir).unwrap();
        assert_eq!(h.owner_pid, std::process::id());
    }

    #[test]
    fn drop_removes_our_lock() {
        let tmp = TempDir::new().unwrap();
        let watch_path = tmp.path().join("target");
        let lock_dir = tmp.path().join("locks");
        let lock_path;
        {
            let h = try_acquire_lock_in("w1", &watch_path, &lock_dir).unwrap();
            lock_path = h.lock_path.clone();
            assert!(lock_path.exists());
        }
        assert!(!lock_path.exists(), "Drop should clean up our own lock");
    }

    #[test]
    fn explicit_release_also_removes_file() {
        let tmp = TempDir::new().unwrap();
        let watch_path = tmp.path().join("target");
        let lock_dir = tmp.path().join("locks");
        let h = try_acquire_lock_in("w1", &watch_path, &lock_dir).unwrap();
        let lock_path = h.lock_path.clone();
        h.release().unwrap();
        assert!(!lock_path.exists());
    }

    #[test]
    fn fnv1a_is_deterministic() {
        let a = fnv1a_hex("/home/user/Desktop");
        let b = fnv1a_hex("/home/user/Desktop");
        assert_eq!(a, b);
        let c = fnv1a_hex("/home/user/Downloads");
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
    }

    #[cfg(unix)]
    #[test]
    fn live_process_is_detected_as_alive() {
        assert!(is_pid_alive(std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn pid_zero_is_not_alive() {
        assert!(!is_pid_alive(0));
    }

    // =====================
    // Glob + file listing
    // =====================

    #[test]
    fn glob_matches_extensions() {
        assert!(glob_match("*.png", "screenshot.png"));
        assert!(glob_match("*.png", ".png"));
        assert!(!glob_match("*.png", "screenshot.jpg"));
    }

    #[test]
    fn glob_matches_prefix_and_suffix() {
        assert!(glob_match("screenshot-*.png", "screenshot-2024-04-19.png"));
        assert!(!glob_match("screenshot-*.png", "shot.png"));
    }

    #[test]
    fn matches_any_is_case_insensitive() {
        let pats = vec!["*.png".to_string()];
        assert!(matches_any(&pats, "Shot.PNG"));
        assert!(matches_any(&pats, "shot.png"));
    }

    #[test]
    fn matches_any_empty_patterns_matches_all() {
        assert!(matches_any(&[], "anything.txt"));
    }

    #[test]
    fn list_files_filters_by_patterns() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.png"), b"x").unwrap();
        fs::write(tmp.path().join("b.jpg"), b"x").unwrap();
        fs::write(tmp.path().join("c.txt"), b"x").unwrap();
        let pats = vec!["*.png".into(), "*.jpg".into()];
        let mut found: Vec<_> = list_files(tmp.path(), &pats, false)
            .into_iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        found.sort();
        assert_eq!(found, vec!["a.png", "b.jpg"]);
    }

    #[test]
    fn list_files_recursive_walks_subdirs() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("root.png"), b"x").unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub/child.png"), b"x").unwrap();
        let pats = vec!["*.png".into()];
        let non_recursive = list_files(tmp.path(), &pats, false);
        assert_eq!(non_recursive.len(), 1);
        let recursive = list_files(tmp.path(), &pats, true);
        assert_eq!(recursive.len(), 2);
    }

    // =====================
    // Poller end-to-end
    // =====================

    fn test_watch(name: &str, path: &Path) -> WatchConfig {
        WatchConfig {
            name: name.into(),
            path: path.to_string_lossy().into_owned(),
            targets: vec!["dev".into()],
            patterns: vec!["*.png".into()],
            catchup: crate::target::CatchupMode::Ignore,
            enabled: true,
            debounce_ms: 80,
            recursive: false,
            on_conflict: crate::target::ConflictAction::Rename,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn poller_emits_new_file_after_debounce() {
        let tmp = TempDir::new().unwrap();
        let lock_dir = tmp.path().join("locks");
        let watch_dir = tmp.path().join("target");
        fs::create_dir_all(&watch_dir).unwrap();

        let lock = try_acquire_lock_in("t1", &watch_dir, &lock_dir).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WatchEvent>();
        let cfg = test_watch("t1", &watch_dir);
        let handle =
            start_poller_with_interval(cfg, lock, tx, 0, std::time::Duration::from_millis(30));

        // Let the poller take its baseline tick.
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        // Drop a new file in.
        fs::write(watch_dir.join("shot.png"), b"hello").unwrap();

        // Wait for the debounce (80ms) plus a couple of polls.
        let event = tokio::time::timeout(std::time::Duration::from_millis(800), rx.recv())
            .await
            .expect("timed out waiting for NewFile")
            .expect("channel closed");
        match event {
            WatchEvent::NewFile {
                watch_name,
                path,
                size,
            } => {
                assert_eq!(watch_name, "t1");
                assert!(path.ends_with("shot.png"));
                assert_eq!(size, 5);
            }
            other => panic!("expected NewFile, got {other:?}"),
        }
        handle.stop();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn poller_ignores_files_already_present() {
        let tmp = TempDir::new().unwrap();
        let lock_dir = tmp.path().join("locks");
        let watch_dir = tmp.path().join("target");
        fs::create_dir_all(&watch_dir).unwrap();
        // Seed a file before starting the poller.
        fs::write(watch_dir.join("pre-existing.png"), b"x").unwrap();

        let lock = try_acquire_lock_in("t2", &watch_dir, &lock_dir).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WatchEvent>();
        let cfg = test_watch("t2", &watch_dir);
        let handle =
            start_poller_with_interval(cfg, lock, tx, 0, std::time::Duration::from_millis(30));

        // Give the poller enough time to have emitted, if it were going to.
        let res = tokio::time::timeout(std::time::Duration::from_millis(400), rx.recv()).await;
        assert!(res.is_err(), "pre-existing files must not be emitted");
        handle.stop();
    }

    #[test]
    fn partition_catchup_splits_by_mtime() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.png"), b"x").unwrap();
        fs::write(tmp.path().join("b.png"), b"x").unwrap();
        let pats = vec!["*.png".into()];
        // threshold=0 → everything is "newer"
        let (newer, older) = partition_catchup(tmp.path(), &pats, false, 0);
        assert_eq!(newer.len(), 2);
        assert_eq!(older.len(), 0);
        // threshold=u64::MAX → everything is "older"
        let (newer, older) = partition_catchup(tmp.path(), &pats, false, u64::MAX);
        assert_eq!(newer.len(), 0);
        assert_eq!(older.len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn poller_catchup_auto_resends_everything_when_threshold_is_zero() {
        let tmp = TempDir::new().unwrap();
        let lock_dir = tmp.path().join("locks");
        let watch_dir = tmp.path().join("target");
        fs::create_dir_all(&watch_dir).unwrap();
        fs::write(watch_dir.join("caught.png"), b"x").unwrap();

        let mut cfg = test_watch("caught", &watch_dir);
        cfg.catchup = crate::target::CatchupMode::Auto;
        let lock = try_acquire_lock_in("caught", &watch_dir, &lock_dir).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WatchEvent>();
        let handle = start_poller_with_interval(
            cfg,
            lock,
            tx,
            0, // everything newer than 0 → auto-emit
            std::time::Duration::from_millis(30),
        );

        let event = tokio::time::timeout(std::time::Duration::from_millis(400), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        match event {
            WatchEvent::NewFile { path, .. } => assert!(path.ends_with("caught.png")),
            other => panic!("expected NewFile, got {other:?}"),
        }
        handle.stop();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn poller_catchup_prompt_emits_detected_event() {
        let tmp = TempDir::new().unwrap();
        let lock_dir = tmp.path().join("locks");
        let watch_dir = tmp.path().join("target");
        fs::create_dir_all(&watch_dir).unwrap();
        fs::write(watch_dir.join("pending.png"), b"x").unwrap();

        let mut cfg = test_watch("prompt", &watch_dir);
        cfg.catchup = crate::target::CatchupMode::Prompt;
        let lock = try_acquire_lock_in("prompt", &watch_dir, &lock_dir).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WatchEvent>();
        let handle = start_poller_with_interval(
            cfg,
            lock,
            tx,
            0, // everything is "newer than 0"
            std::time::Duration::from_millis(30),
        );

        let event = tokio::time::timeout(std::time::Duration::from_millis(400), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        match event {
            WatchEvent::CatchupDetected { watch_name, paths } => {
                assert_eq!(watch_name, "prompt");
                assert_eq!(paths.len(), 1);
                assert!(paths[0].ends_with("pending.png"));
            }
            other => panic!("expected CatchupDetected, got {other:?}"),
        }
        handle.stop();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn poller_filters_by_pattern() {
        let tmp = TempDir::new().unwrap();
        let lock_dir = tmp.path().join("locks");
        let watch_dir = tmp.path().join("target");
        fs::create_dir_all(&watch_dir).unwrap();

        let lock = try_acquire_lock_in("t3", &watch_dir, &lock_dir).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WatchEvent>();
        let cfg = test_watch("t3", &watch_dir);
        let handle =
            start_poller_with_interval(cfg, lock, tx, 0, std::time::Duration::from_millis(30));

        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        // A .txt file must be ignored.
        fs::write(watch_dir.join("note.txt"), b"hi").unwrap();

        let res = tokio::time::timeout(std::time::Duration::from_millis(400), rx.recv()).await;
        assert!(res.is_err(), "non-matching files must not be emitted");
        handle.stop();
    }
}
