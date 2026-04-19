use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::history::{HistoryEntry, HistoryStatus, HistoryStore};
use crate::path_input;
use crate::target::{ClipboardFormat, SyncMode};
use crate::transfer::{self, Transfer, TransferEvent, TransferState};

#[derive(Debug, Clone)]
pub enum AppEvent {
    Terminal(Event),
    TransferUpdate(TransferEvent),
    UpdateCheckResult(std::result::Result<Option<String>, String>),
    UpdateInstallResult(std::result::Result<Vec<PathBuf>, String>),
    PreflightResult {
        target_name: String,
        status: TargetStatus,
    },
    RemoteInstallResult {
        target_name: String,
        result: std::result::Result<(), String>,
    },
    RsyncVersionWarning(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    Available(String),  // newer version
    Installing(String), // version being installed
    Installed(Vec<PathBuf>),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    DropZone,
    Targets,
    Progress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // some actions are available via menu only, not the help bar
pub enum HelpBarAction {
    CycleFocus,
    ToggleSelection,
    Sync,
    CycleMode,
    CycleClipboard,
    CheckUpdate,
    ToggleHelp,
    OpenMenu,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    AddTargetFromSsh,
    RemoveCurrentTarget,
    ReprobeTargets,
    InstallRsyncOnSelected,
    ClearProgressHistory,
    ToggleMode,
    CycleClipboardFormat,
    CycleTheme,
    ClearQueue,
    CheckUpdate,
    Help,
    Quit,
}

impl MenuAction {
    pub fn label(self) -> &'static str {
        match self {
            MenuAction::AddTargetFromSsh => "Add target from ~/.ssh/config",
            MenuAction::RemoveCurrentTarget => "Remove selected target",
            MenuAction::ReprobeTargets => "Re-probe all targets",
            MenuAction::InstallRsyncOnSelected => "Install rsync on selected target",
            MenuAction::ClearProgressHistory => "Clear transfer history",
            MenuAction::ToggleMode => "Toggle auto / manual mode",
            MenuAction::CycleClipboardFormat => "Cycle clipboard format",
            MenuAction::CycleTheme => "Cycle theme",
            MenuAction::ClearQueue => "Clear queue",
            MenuAction::CheckUpdate => "Check for update",
            MenuAction::Help => "Show help",
            MenuAction::Quit => "Quit",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            MenuAction::AddTargetFromSsh => "",
            MenuAction::RemoveCurrentTarget => "",
            MenuAction::ReprobeTargets => "",
            MenuAction::InstallRsyncOnSelected => "",
            MenuAction::ClearProgressHistory => "",
            MenuAction::ToggleMode => "^A / ^N",
            MenuAction::CycleClipboardFormat => "^F",
            MenuAction::CycleTheme => "^T",
            MenuAction::ClearQueue => "",
            MenuAction::CheckUpdate => "^U",
            MenuAction::Help => "^H",
            MenuAction::Quit => "^Q",
        }
    }

    pub fn all() -> &'static [MenuAction] {
        &[
            MenuAction::AddTargetFromSsh,
            MenuAction::RemoveCurrentTarget,
            MenuAction::ReprobeTargets,
            MenuAction::InstallRsyncOnSelected,
            MenuAction::ClearProgressHistory,
            MenuAction::ToggleMode,
            MenuAction::CycleClipboardFormat,
            MenuAction::CycleTheme,
            MenuAction::ClearQueue,
            MenuAction::CheckUpdate,
            MenuAction::Help,
            MenuAction::Quit,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalHit {
    Confirm,
    Cancel,
    Dismiss,
}

#[derive(Debug, Default, Clone)]
pub struct HitRegions {
    pub drop_zone: Rect,
    pub queue_rows: Vec<Rect>,
    pub targets_panel: Rect,
    pub target_rows: Vec<Rect>,
    pub progress_panel: Rect,
    /// Clickable Activity rows — (area, cursor index into `activity_row_refs`,
    /// what the row represents).
    pub activity_rows: Vec<(Rect, usize, ActivityRef)>,
    pub help_bar_hits: Vec<(Rect, HelpBarAction)>,
    pub modal_area: Option<Rect>,
    pub modal_hits: Vec<(Rect, ModalHit)>,
    pub menu_rows: Vec<(Rect, MenuAction)>,
    pub ssh_picker_rows: Vec<Rect>,
}

fn rect_contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Single,
    Group,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TargetStatus {
    #[default]
    Unknown,
    Probing,
    Reachable,
    NoRsync,
    Unreachable(String),
}

#[derive(Debug, Clone)]
pub struct TargetRow {
    pub name: String,
    pub kind: TargetKind,
    pub summary: String,
    pub selected: bool,
    pub status: TargetStatus,
}

#[derive(Debug, Clone)]
pub struct SshPicker {
    pub list: Vec<crate::ssh_config::SshHost>,
    pub cursor: usize,
}

/// Identity of a row rendered in the Activity panel. Lets the key handler
/// act on the renderer's exact ordering (live transfers first, then merged
/// history groups).
#[derive(Debug, Clone)]
pub enum ActivityRef {
    Live(u64),
    /// Index into the filtered history list; refers to the REPRESENTATIVE
    /// entry of a merged group (same remote_path collapses multiple targets
    /// into one row).
    History(usize),
}

pub struct App {
    pub cfg: Config,
    pub mode: SyncMode,
    pub clipboard_format: ClipboardFormat,
    pub focus: Focus,
    pub should_quit: bool,

    pub target_rows: Vec<TargetRow>,
    pub target_cursor: usize,

    pub queue: Vec<PathBuf>,
    pub queue_cursor: usize,
    pub last_paste_error: Option<String>,

    pub transfers: Vec<Transfer>,
    pub transfer_index: HashMap<u64, usize>,
    pub next_transfer_id: u64,

    pub last_clipboard: Option<String>,
    pub toast: Option<(String, std::time::Instant)>,
    pub help_visible: bool,
    pub menu_visible: bool,
    pub menu_cursor: usize,

    pub ssh_picker: Option<SshPicker>,
    /// None = inactive. Some("") = filter bar visible but no query. Some(q) =
    /// filtering the activity list by q. Lives with Focus::Progress.
    pub activity_filter: Option<String>,
    /// Cursor index into the combined Activity row list (live transfers
    /// first, then merged history groups). Clamped every frame by the
    /// renderer so it can never point past the end.
    pub activity_cursor: usize,
    /// Remote path text currently on the system clipboard. Rendered as a
    /// persistent `📋 on clipboard` suffix on any Activity row whose
    /// remote_path matches, so you can always tell what's about to paste.
    pub last_copied_remote: Option<String>,
    pub history: HistoryStore,

    pub update_status: UpdateStatus,
    pub hit_regions: HitRegions,

    pub transfer_tx: mpsc::UnboundedSender<TransferEvent>,
    pub transfer_rx: mpsc::UnboundedReceiver<TransferEvent>,
    pub app_tx: mpsc::UnboundedSender<AppEvent>,
    pub app_rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl App {
    pub fn new(cfg: Config) -> Self {
        let mut rows: Vec<TargetRow> = cfg
            .targets
            .iter()
            .map(|t| TargetRow {
                name: t.name.clone(),
                kind: TargetKind::Single,
                summary: t.display_endpoint(),
                selected: false,
                status: TargetStatus::Unknown,
            })
            .collect();
        for g in &cfg.groups {
            rows.push(TargetRow {
                name: g.name.clone(),
                kind: TargetKind::Group,
                summary: format!("group → {}", g.targets.join(" + ")),
                selected: false,
                status: TargetStatus::Unknown,
            });
        }

        // Default selection: prefer configured default_target, else first row.
        if let Some(name) = &cfg.default_target {
            if let Some(r) = rows.iter_mut().find(|r| &r.name == name) {
                r.selected = true;
            }
        } else if let Some(first) = rows.first_mut() {
            first.selected = true;
        }

        let mode = cfg.default_mode;
        let clipboard_format = cfg.clipboard_format;
        let (tx, rx) = mpsc::unbounded_channel();
        let (app_tx, app_rx) = mpsc::unbounded_channel();

        Self {
            cfg,
            mode,
            clipboard_format,
            focus: Focus::DropZone,
            should_quit: false,
            target_rows: rows,
            target_cursor: 0,
            queue: vec![],
            queue_cursor: 0,
            last_paste_error: None,
            transfers: vec![],
            transfer_index: HashMap::new(),
            next_transfer_id: 1,
            last_clipboard: None,
            toast: None,
            help_visible: false,
            menu_visible: false,
            menu_cursor: 0,
            ssh_picker: None,
            activity_filter: None,
            activity_cursor: 0,
            last_copied_remote: None,
            history: HistoryStore::load().unwrap_or_default(),
            update_status: UpdateStatus::Idle,
            hit_regions: HitRegions::default(),
            transfer_tx: tx,
            transfer_rx: rx,
            app_tx,
            app_rx,
        }
    }

    pub fn tick(&mut self) {
        if let Some((_, at)) = &self.toast
            && at.elapsed() > std::time::Duration::from_secs(4)
        {
            self.toast = None;
        }
    }

    pub fn handle_event(&mut self, evt: AppEvent) {
        match evt {
            AppEvent::Terminal(Event::Key(k)) => self.handle_key(k),
            AppEvent::Terminal(Event::Paste(s)) => self.handle_paste(&s),
            AppEvent::Terminal(Event::Mouse(m)) => self.handle_mouse(m),
            AppEvent::Terminal(_) => {}
            AppEvent::TransferUpdate(u) => self.handle_transfer_event(u),
            AppEvent::UpdateCheckResult(r) => self.handle_update_check_result(r),
            AppEvent::UpdateInstallResult(r) => self.handle_update_install_result(r),
            AppEvent::PreflightResult {
                target_name,
                status,
            } => self.apply_preflight_result(&target_name, status),
            AppEvent::RemoteInstallResult {
                target_name,
                result,
            } => self.apply_remote_install_result(&target_name, result),
            AppEvent::RsyncVersionWarning(msg) => self.toast(&msg),
        }
    }

    /// Probe the local rsync version once at startup so we can warn the user
    /// if it's ancient — macOS ships rsync 2.6.9 which lacks many useful
    /// flags and tends to bite people exactly in this workflow.
    pub fn spawn_rsync_version_check(&mut self) {
        let tx = self.app_tx.clone();
        tokio::spawn(async move {
            let (a, b, _c) = crate::transfer::local_rsync_version().await;
            if a == 0 {
                let _ = tx.send(AppEvent::RsyncVersionWarning(
                    "rsync not found locally — install it: brew install rsync".to_string(),
                ));
            } else if (a, b) < (3, 0) {
                let _ = tx.send(AppEvent::RsyncVersionWarning(format!(
                    "local rsync is v{a}.{b} (old); upgrade: brew install rsync"
                )));
            }
        });
    }

    pub fn spawn_preflight_all(&mut self) {
        if self.cfg.targets.is_empty() {
            return;
        }
        for row in &mut self.target_rows {
            if row.kind == TargetKind::Group {
                continue;
            }
            row.status = TargetStatus::Probing;
        }
        for target in self.cfg.targets.clone() {
            let tx = self.app_tx.clone();
            let name = target.name.clone();
            tokio::spawn(async move {
                let status = match crate::transfer::preflight_full(&target).await {
                    Ok(_) => TargetStatus::Reachable,
                    Err(e) => {
                        let msg = format!("{e:#}");
                        if msg.contains("rsync not found on remote") {
                            TargetStatus::NoRsync
                        } else {
                            TargetStatus::Unreachable(msg)
                        }
                    }
                };
                let _ = tx.send(AppEvent::PreflightResult {
                    target_name: name,
                    status,
                });
            });
        }
    }

    fn apply_preflight_result(&mut self, name: &str, status: TargetStatus) {
        if let Some(row) = self.target_rows.iter_mut().find(|r| r.name == name) {
            row.status = status;
        }
    }

    pub fn start_remote_rsync_install(&mut self, target_name: &str) {
        let Some(target) = self.cfg.target_by_name(target_name).cloned() else {
            return;
        };
        if let Some(row) = self.target_rows.iter_mut().find(|r| r.name == target_name) {
            row.status = TargetStatus::Probing;
        }
        self.toast(&format!("installing rsync on {target_name}…"));
        let tx = self.app_tx.clone();
        let name = target_name.to_string();
        tokio::spawn(async move {
            let result = crate::transfer::remote_install_rsync(&target)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppEvent::RemoteInstallResult {
                target_name: name,
                result,
            });
        });
    }

    fn apply_remote_install_result(&mut self, name: &str, result: std::result::Result<(), String>) {
        match result {
            Ok(()) => {
                self.toast(&format!("installed rsync on {name}"));
                // Re-probe that target.
                if let Some(target) = self.cfg.target_by_name(name).cloned() {
                    if let Some(row) = self.target_rows.iter_mut().find(|r| r.name == name) {
                        row.status = TargetStatus::Probing;
                    }
                    let tx = self.app_tx.clone();
                    let tname = name.to_string();
                    tokio::spawn(async move {
                        let status = match crate::transfer::preflight_full(&target).await {
                            Ok(_) => TargetStatus::Reachable,
                            Err(e) => TargetStatus::Unreachable(format!("{e:#}")),
                        };
                        let _ = tx.send(AppEvent::PreflightResult {
                            target_name: tname,
                            status,
                        });
                    });
                }
            }
            Err(e) => {
                if let Some(row) = self.target_rows.iter_mut().find(|r| r.name == name) {
                    row.status = TargetStatus::NoRsync;
                }
                self.toast(&format!("install failed: {e}"));
            }
        }
    }

    fn handle_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollDown => self.move_cursor(1),
            MouseEventKind::ScrollUp => self.move_cursor(-1),
            MouseEventKind::Down(MouseButton::Left) => self.handle_click(m.column, m.row),
            _ => {}
        }
    }

    fn handle_click(&mut self, x: u16, y: u16) {
        if self.ssh_picker.is_some() {
            let rows = self.hit_regions.ssh_picker_rows.clone();
            for (i, r) in rows.iter().enumerate() {
                if rect_contains(*r, x, y) {
                    if let Some(picker) = self.ssh_picker.as_mut() {
                        picker.cursor = i;
                    }
                    self.add_picker_selection();
                    return;
                }
            }
            // Click outside picker dismisses.
            self.ssh_picker = None;
            return;
        }
        // Menu takes precedence (it's drawn on top of everything else).
        if self.menu_visible {
            for (r, action) in self.hit_regions.menu_rows.clone() {
                if rect_contains(r, x, y) {
                    self.menu_visible = false;
                    self.run_menu_action(action);
                    return;
                }
            }
            // Click outside menu rows → dismiss.
            self.menu_visible = false;
            return;
        }

        // Modal takes precedence.
        if let Some(modal) = self.hit_regions.modal_area
            && rect_contains(modal, x, y)
        {
            for (r, hit) in &self.hit_regions.modal_hits.clone() {
                if rect_contains(*r, x, y) {
                    match hit {
                        ModalHit::Confirm => {
                            if let UpdateStatus::Available(v) = &self.update_status {
                                let ver = v.clone();
                                self.start_update_install(ver);
                            }
                        }
                        ModalHit::Cancel | ModalHit::Dismiss => {
                            self.update_status = UpdateStatus::Idle;
                        }
                    }
                    return;
                }
            }
            // Click inside Installed / Failed modal body but not on a specific
            // hit zone — treat as "dismiss".
            if matches!(
                self.update_status,
                UpdateStatus::Installed(_) | UpdateStatus::Failed(_)
            ) {
                self.update_status = UpdateStatus::Idle;
            }
            return;
        }

        if self.help_visible {
            // Any click outside/inside the overlay dismisses it.
            self.help_visible = false;
            return;
        }

        // Help-bar chips.
        for (r, action) in &self.hit_regions.help_bar_hits.clone() {
            if rect_contains(*r, x, y) {
                self.trigger_help_bar_action(*action);
                return;
            }
        }

        // Target row click → focus targets. Behaviour depends on status:
        //   [⚠] NoRsync     → trigger auto-install
        //   [✗] Unreachable → toast the full ssh error so user can diagnose
        //   otherwise       → toggle selection
        for (i, r) in self.hit_regions.target_rows.clone().iter().enumerate() {
            if rect_contains(*r, x, y) {
                self.focus = Focus::Targets;
                self.target_cursor = i;
                if let Some(row) = self.target_rows.get(i) {
                    match &row.status {
                        TargetStatus::NoRsync => {
                            let name = row.name.clone();
                            self.start_remote_rsync_install(&name);
                            return;
                        }
                        TargetStatus::Unreachable(e) => {
                            let msg = e.lines().next().unwrap_or("").trim().to_string();
                            self.toast(&msg);
                            return;
                        }
                        _ => {}
                    }
                }
                if i < self.target_rows.len() {
                    self.toggle_target_cursor();
                }
                return;
            }
        }

        // Queue row click → focus drop zone + move cursor.
        for (i, r) in self.hit_regions.queue_rows.clone().iter().enumerate() {
            if rect_contains(*r, x, y) {
                self.focus = Focus::DropZone;
                if i < self.queue.len() {
                    self.queue_cursor = i;
                }
                return;
            }
        }

        // Activity row click → copy the row's remote path and move the
        // cursor to it so the highlight reflects what's on the clipboard.
        for (r, cursor_idx, act) in self.hit_regions.activity_rows.clone() {
            if rect_contains(r, x, y) {
                self.focus = Focus::Progress;
                self.activity_cursor = cursor_idx;
                match act {
                    ActivityRef::Live(id) => self.recopy_completed(id),
                    ActivityRef::History(idx) => self.copy_history_by_index(idx),
                }
                return;
            }
        }

        // Panel background click → focus that panel.
        if rect_contains(self.hit_regions.drop_zone, x, y) {
            self.focus = Focus::DropZone;
        } else if rect_contains(self.hit_regions.targets_panel, x, y) {
            self.focus = Focus::Targets;
        } else if rect_contains(self.hit_regions.progress_panel, x, y) {
            self.focus = Focus::Progress;
        }
    }

    fn trigger_help_bar_action(&mut self, a: HelpBarAction) {
        match a {
            HelpBarAction::CycleFocus => self.cycle_focus(1),
            HelpBarAction::ToggleSelection => {
                if self.focus == Focus::Targets {
                    self.toggle_target_cursor();
                }
            }
            HelpBarAction::Sync => self.start_queue_sync(),
            HelpBarAction::CycleMode => self.set_mode(match self.mode {
                SyncMode::Auto => SyncMode::Manual,
                SyncMode::Manual => SyncMode::Auto,
            }),
            HelpBarAction::CycleClipboard => self.cycle_clipboard_format(),
            HelpBarAction::CheckUpdate => self.start_update_check(),
            HelpBarAction::ToggleHelp => self.help_visible = !self.help_visible,
            HelpBarAction::OpenMenu => {
                self.menu_visible = true;
                self.menu_cursor = 0;
            }
            HelpBarAction::Quit => self.should_quit = true,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        // Ctrl+C always quits, even through modals.
        if let KeyCode::Char('c') = key.code
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.should_quit = true;
            return;
        }
        // Update confirm modal swallows keys: y confirms, n/Esc dismisses.
        if let UpdateStatus::Available(ref v) = self.update_status {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let ver = v.clone();
                    self.start_update_install(ver);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.update_status = UpdateStatus::Idle;
                }
                _ => {}
            }
            return;
        }
        // Dismissable terminal states: any key clears.
        if matches!(
            self.update_status,
            UpdateStatus::Installed(_) | UpdateStatus::Failed(_)
        ) {
            self.update_status = UpdateStatus::Idle;
            return;
        }
        if self.ssh_picker.is_some() {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.ssh_picker = None;
                }
                KeyCode::Up => self.picker_move(-1),
                KeyCode::Down | KeyCode::Tab => self.picker_move(1),
                KeyCode::Enter => self.add_picker_selection(),
                _ => {}
            }
            return;
        }
        if self.menu_visible {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.menu_visible = false;
                }
                KeyCode::Up => {
                    let n = MenuAction::all().len();
                    self.menu_cursor = (self.menu_cursor + n - 1) % n;
                }
                KeyCode::Down | KeyCode::Tab => {
                    let n = MenuAction::all().len();
                    self.menu_cursor = (self.menu_cursor + 1) % n;
                }
                KeyCode::Enter => {
                    let action = MenuAction::all()[self.menu_cursor];
                    self.menu_visible = false;
                    self.run_menu_action(action);
                }
                KeyCode::Char(d @ '1'..='9') => {
                    let idx = (d as u8 - b'1') as usize;
                    if idx < MenuAction::all().len() {
                        let action = MenuAction::all()[idx];
                        self.menu_visible = false;
                        self.run_menu_action(action);
                    }
                }
                _ => {}
            }
            return;
        }
        if self.help_visible {
            self.help_visible = false;
            return;
        }
        // Activity filter input swallows text while active.
        if self.activity_filter.is_some() && self.focus == Focus::Progress {
            match key.code {
                KeyCode::Esc => {
                    self.activity_filter = None;
                }
                KeyCode::Backspace => {
                    if let Some(q) = self.activity_filter.as_mut() {
                        q.pop();
                    }
                }
                KeyCode::Enter => {
                    self.activity_filter = None;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(q) = self.activity_filter.as_mut() {
                        q.push(c);
                    }
                }
                _ => {}
            }
            return;
        }
        // Every global shortcut is Ctrl+letter. Paths dragged into the
        // terminal as raw characters can't contain Ctrl modifiers, so this
        // is the only layer that never collides with drag-and-drop.
        // Plain letters / digits / `?` are free for paste and filter input.
        match key.code {
            KeyCode::Tab => self.cycle_focus(1),
            KeyCode::BackTab => self.cycle_focus(-1),
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Enter => {
                if self.focus == Focus::Progress {
                    self.copy_activity_cursor();
                } else {
                    self.start_queue_sync();
                }
            }
            KeyCode::Delete | KeyCode::Backspace if self.focus == Focus::DropZone => {
                if !self.queue.is_empty() && self.queue_cursor < self.queue.len() {
                    self.queue.remove(self.queue_cursor);
                    if self.queue_cursor >= self.queue.len() && self.queue_cursor > 0 {
                        self.queue_cursor -= 1;
                    }
                }
            }
            // Ctrl+letter shortcuts. Ctrl+C is handled earlier in this
            // function as "quit", so it's not listed here.
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => match c {
                'p' => {
                    self.menu_visible = true;
                    self.menu_cursor = 0;
                }
                'q' => self.should_quit = true,
                'h' => self.help_visible = !self.help_visible,
                'u' => self.start_update_check(),
                't' => self.cycle_theme(),
                'a' => self.set_mode(SyncMode::Auto),
                'n' => self.set_mode(SyncMode::Manual),
                'f' => {
                    if self.focus == Focus::Progress {
                        self.activity_filter = Some(String::new());
                    } else {
                        self.cycle_clipboard_format();
                    }
                }
                _ => {}
            },
            // Space / 1–9 still act as Targets-panel conveniences — they
            // fire ONLY when Targets is focused, so a path dropped into the
            // DropZone can't toggle anything.
            KeyCode::Char(' ') if self.focus == Focus::Targets => self.toggle_target_cursor(),
            KeyCode::Char(d @ '1'..='9') if self.focus == Focus::Targets => {
                let idx = (d as u8 - b'1') as usize;
                if idx < self.target_rows.len() {
                    self.target_cursor = idx;
                    self.toggle_target_cursor();
                }
            }
            _ => {}
        }
    }

    fn set_mode(&mut self, m: SyncMode) {
        self.mode = m;
        self.cfg.default_mode = m;
        self.toast(&format!("mode: {:?}", m).to_lowercase());
        self.persist_config();
    }

    fn cycle_theme(&mut self) {
        let themes = ["mocha", "tokyo_night", "gruvbox", "nord", "dracula"];
        let cur = themes
            .iter()
            .position(|t| *t == self.cfg.theme)
            .unwrap_or(0);
        let next = (cur + 1) % themes.len();
        self.cfg.theme = themes[next].to_string();
        self.toast(&format!("theme: {}", themes[next]));
        // Persist so the next launch boots with the user's last pick. Same
        // for clipboard format / mode below.
        self.persist_config();
    }

    fn run_menu_action(&mut self, a: MenuAction) {
        match a {
            MenuAction::AddTargetFromSsh => self.open_ssh_picker(),
            MenuAction::RemoveCurrentTarget => self.remove_current_target(),
            MenuAction::CheckUpdate => self.start_update_check(),
            MenuAction::ReprobeTargets => {
                self.spawn_preflight_all();
                self.toast("re-probing targets…");
            }
            MenuAction::InstallRsyncOnSelected => {
                if let Some(row) = self.target_rows.get(self.target_cursor) {
                    if row.kind == TargetKind::Single {
                        let name = row.name.clone();
                        self.start_remote_rsync_install(&name);
                    } else {
                        self.toast("select a single target first");
                    }
                }
            }
            MenuAction::ClearProgressHistory => {
                let n = self.transfers.len();
                self.transfers.clear();
                self.transfer_index.clear();
                self.toast(&format!("cleared {n} transfer(s)"));
            }
            MenuAction::ToggleMode => self.set_mode(match self.mode {
                SyncMode::Auto => SyncMode::Manual,
                SyncMode::Manual => SyncMode::Auto,
            }),
            MenuAction::CycleClipboardFormat => self.cycle_clipboard_format(),
            MenuAction::CycleTheme => self.cycle_theme(),
            MenuAction::ClearQueue => {
                let n = self.queue.len();
                self.queue.clear();
                self.queue_cursor = 0;
                self.toast(&format!("cleared {n} queued"));
            }
            MenuAction::Help => self.help_visible = true,
            MenuAction::Quit => self.should_quit = true,
        }
    }

    /// Filter string used by the Activity panel (lower-cased, trimmed). Empty
    /// when the user hasn't entered filter mode.
    pub fn activity_query(&self) -> String {
        self.activity_filter.clone().unwrap_or_default()
    }

    /// Copy the remote path for the history entry at `idx` in the currently
    /// filtered view. Called from the Activity row click handler.
    pub fn copy_history_by_index(&mut self, idx: usize) {
        let q = self.activity_query();
        let matches = self.history.filter(&q);
        let Some(entry) = matches.get(idx).cloned().cloned() else {
            return;
        };
        let text = if let Some(t) = self.cfg.target_by_name(&entry.target_name) {
            let dir = std::path::Path::new(&entry.remote_path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            crate::clipboard::render(
                t,
                std::path::Path::new(&entry.local_path),
                &dir,
                self.clipboard_format,
                &self.cfg.clipboard_template,
            )
        } else {
            entry.remote_path.clone()
        };
        self.write_clipboard_text(text, Some(entry.remote_path));
    }

    /// Write `text` to the system clipboard, update the toast, and remember
    /// which remote path it corresponds to so the Activity panel can mark
    /// that row with a persistent "on clipboard" badge.
    fn write_clipboard_text(&mut self, text: String, remote_path: Option<String>) {
        match crate::clipboard::write(&text) {
            Ok(()) => {
                self.last_clipboard = Some(text.clone());
                self.last_copied_remote = remote_path;
                self.toast(&format!("clipboard ← {}", trunc(&text, 60)));
            }
            Err(e) => {
                self.toast(&format!("clipboard error: {e}"));
            }
        }
    }

    /// Keyboard: copy whichever Activity row the cursor is on. Indices are
    /// resolved against the exact ordering the renderer produced last frame
    /// via `activity_row_refs`.
    pub fn copy_activity_cursor(&mut self) {
        let refs = self.activity_row_refs();
        if refs.is_empty() {
            return;
        }
        let cursor = self.activity_cursor.min(refs.len() - 1);
        match refs[cursor].clone() {
            ActivityRef::Live(id) => self.recopy_completed(id),
            ActivityRef::History(idx) => self.copy_history_by_index(idx),
        }
    }

    /// Compute the row-identity list the Activity renderer uses. Lives in
    /// app.rs so the key handler (no access to the UI module) can resolve
    /// the cursor position the same way.
    pub fn activity_row_refs(&self) -> Vec<ActivityRef> {
        let q = self.activity_query();
        let q_lower = q.to_lowercase();
        let mut out = Vec::new();
        // Live = anything not yet in history.
        for t in self.transfers.iter().rev() {
            if t.state == TransferState::Completed {
                continue;
            }
            if !q_lower.is_empty() {
                let fname = t
                    .local
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !fname.to_lowercase().contains(&q_lower)
                    && !t.target_name.to_lowercase().contains(&q_lower)
                {
                    continue;
                }
            }
            out.push(ActivityRef::Live(t.id));
        }
        // History — merged by remote_path. The rep is the FIRST (newest)
        // entry seen for a given remote_path in the filtered list.
        let matches = self.history.filter(&q);
        let mut seen: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
        for (i, e) in matches.iter().enumerate() {
            if seen.contains_key(&e.remote_path) {
                continue;
            }
            seen.insert(e.remote_path.clone(), ());
            out.push(ActivityRef::History(i));
        }
        out
    }

    pub fn move_activity_cursor(&mut self, delta: i32) {
        let n = self.activity_row_refs().len();
        if n == 0 {
            self.activity_cursor = 0;
            return;
        }
        let cur = self.activity_cursor.min(n - 1) as i32;
        self.activity_cursor = (cur + delta).rem_euclid(n as i32) as usize;
    }

    fn open_ssh_picker(&mut self) {
        let mut hosts = crate::ssh_config::load();
        // Filter out hosts that are already configured.
        let existing: std::collections::HashSet<String> =
            self.cfg.targets.iter().map(|t| t.name.clone()).collect();
        hosts.retain(|h| !existing.contains(&h.name));
        if hosts.is_empty() {
            self.toast("no SSH hosts found (or all already added)");
            return;
        }
        self.ssh_picker = Some(SshPicker {
            list: hosts,
            cursor: 0,
        });
    }

    pub fn add_picker_selection(&mut self) {
        let Some(picker) = self.ssh_picker.as_ref() else {
            return;
        };
        let Some(h) = picker.list.get(picker.cursor).cloned() else {
            return;
        };
        self.ssh_picker = None;

        let target = crate::config::target_from_ssh_host(h);
        let name = target.name.clone();
        // If this is the first target, mark it as default so it's
        // auto-selected in the UI and the user can start dropping files
        // immediately without another click.
        let was_empty = self.cfg.targets.is_empty();
        self.cfg.targets.push(target.clone());
        if was_empty {
            self.cfg.default_target = Some(name.clone());
        }
        self.rebuild_target_rows();
        self.persist_config();

        // Kick off a preflight for the new target only.
        let tx = self.app_tx.clone();
        tokio::spawn(async move {
            let status = match crate::transfer::preflight_full(&target).await {
                Ok(_) => TargetStatus::Reachable,
                Err(e) => {
                    let msg = format!("{e:#}");
                    if msg.contains("rsync not found on remote") {
                        TargetStatus::NoRsync
                    } else {
                        TargetStatus::Unreachable(msg)
                    }
                }
            };
            let _ = tx.send(AppEvent::PreflightResult {
                target_name: name,
                status,
            });
        });
        self.toast(&format!(
            "added target: {}",
            self.cfg
                .targets
                .last()
                .map(|t| t.name.clone())
                .unwrap_or_default()
        ));
    }

    fn remove_current_target(&mut self) {
        let Some(row) = self.target_rows.get(self.target_cursor).cloned() else {
            return;
        };
        if row.kind != TargetKind::Single {
            self.toast("select a target (not a group) to remove");
            return;
        }
        let name = row.name.clone();
        self.cfg.targets.retain(|t| t.name != name);
        if let Some(dt) = &self.cfg.default_target
            && dt == &name
        {
            self.cfg.default_target = self.cfg.targets.first().map(|t| t.name.clone());
        }
        self.rebuild_target_rows();
        self.persist_config();
        self.toast(&format!("removed target: {name}"));
    }

    fn rebuild_target_rows(&mut self) {
        // Preserve status and selection state for existing rows by name so
        // adding or removing one target doesn't reset probes/selections on
        // unrelated ones.
        let prior: std::collections::HashMap<String, (TargetStatus, bool)> = self
            .target_rows
            .iter()
            .map(|r| (r.name.clone(), (r.status.clone(), r.selected)))
            .collect();

        let mut rows: Vec<TargetRow> = self
            .cfg
            .targets
            .iter()
            .map(|t| {
                let (status, selected) = prior.get(&t.name).cloned().unwrap_or_else(|| {
                    let sel = self
                        .cfg
                        .default_target
                        .as_deref()
                        .map(|d| d == t.name)
                        .unwrap_or(false);
                    (TargetStatus::Unknown, sel)
                });
                TargetRow {
                    name: t.name.clone(),
                    kind: TargetKind::Single,
                    summary: t.display_endpoint(),
                    selected,
                    status,
                }
            })
            .collect();
        for g in &self.cfg.groups {
            let (status, selected) = prior
                .get(&g.name)
                .cloned()
                .unwrap_or((TargetStatus::Unknown, false));
            rows.push(TargetRow {
                name: g.name.clone(),
                kind: TargetKind::Group,
                summary: format!("group → {}", g.targets.join(" + ")),
                selected,
                status,
            });
        }
        self.target_rows = rows;
        if self.target_cursor >= self.target_rows.len() {
            self.target_cursor = self.target_rows.len().saturating_sub(1);
        }
    }

    fn persist_config(&mut self) {
        match crate::config::save_global(&self.cfg) {
            Ok(path) => {
                self.toast(&format!(
                    "saved {}",
                    path.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
            }
            Err(e) => {
                self.toast(&format!("save failed: {e}"));
            }
        }
    }

    pub fn picker_move(&mut self, delta: i32) {
        if let Some(picker) = self.ssh_picker.as_mut() {
            let n = picker.list.len();
            if n == 0 {
                return;
            }
            let cur = picker.cursor as i32;
            picker.cursor = ((cur + delta).rem_euclid(n as i32)) as usize;
        }
    }

    fn start_update_check(&mut self) {
        if matches!(
            self.update_status,
            UpdateStatus::Checking | UpdateStatus::Installing(_)
        ) {
            return;
        }
        self.update_status = UpdateStatus::Checking;
        let tx = self.app_tx.clone();
        tokio::spawn(async move {
            let r = match crate::update::check_for_updates().await {
                Ok(v) => Ok(v),
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = tx.send(AppEvent::UpdateCheckResult(r));
        });
    }

    fn handle_update_check_result(&mut self, r: std::result::Result<Option<String>, String>) {
        match r {
            Ok(Some(v)) => {
                self.update_status = UpdateStatus::Available(v);
            }
            Ok(None) => {
                self.update_status = UpdateStatus::Idle;
                self.toast(&format!("up to date (v{})", crate::VERSION));
            }
            Err(e) => {
                self.update_status = UpdateStatus::Failed(e);
            }
        }
    }

    fn start_update_install(&mut self, version: String) {
        self.update_status = UpdateStatus::Installing(version.clone());
        let tx = self.app_tx.clone();
        tokio::spawn(async move {
            let r = match crate::update::download_and_install(&version).await {
                Ok(paths) => Ok(paths),
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = tx.send(AppEvent::UpdateInstallResult(r));
        });
    }

    fn handle_update_install_result(&mut self, r: std::result::Result<Vec<PathBuf>, String>) {
        match r {
            Ok(paths) => {
                self.update_status = UpdateStatus::Installed(paths);
            }
            Err(e) => {
                self.update_status = UpdateStatus::Failed(e);
            }
        }
    }

    fn handle_paste(&mut self, raw: &str) {
        let parsed = path_input::parse_paste(raw);
        if parsed.is_empty() {
            self.last_paste_error = Some("empty paste".into());
            return;
        }
        let mut accepted = 0;
        let mut missing: Vec<String> = vec![];
        for p in parsed {
            if path_input::path_exists(&p) {
                self.queue.push(PathBuf::from(p));
                accepted += 1;
            } else {
                missing.push(p);
            }
        }
        if accepted > 0 {
            self.last_paste_error = if missing.is_empty() {
                None
            } else {
                Some(format!("{} path(s) not found", missing.len()))
            };
            self.toast(&format!("queued {accepted}"));
            if self.mode == SyncMode::Auto {
                self.start_queue_sync();
            }
        } else {
            self.last_paste_error = Some(format!("no valid path: {}", missing.join(", ")));
        }
    }

    fn cycle_focus(&mut self, dir: i32) {
        let order = [Focus::DropZone, Focus::Targets, Focus::Progress];
        let idx = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let n = order.len() as i32;
        let next = ((idx as i32 + dir).rem_euclid(n)) as usize;
        self.focus = order[next];
    }

    fn move_cursor(&mut self, delta: i32) {
        match self.focus {
            Focus::DropZone => {
                if self.queue.is_empty() {
                    return;
                }
                let len = self.queue.len() as i32;
                let next = (self.queue_cursor as i32 + delta).rem_euclid(len);
                self.queue_cursor = next as usize;
            }
            Focus::Targets => {
                if self.target_rows.is_empty() {
                    return;
                }
                let len = self.target_rows.len() as i32;
                let next = (self.target_cursor as i32 + delta).rem_euclid(len);
                self.target_cursor = next as usize;
            }
            Focus::Progress => self.move_activity_cursor(delta),
        }
    }

    fn toggle_target_cursor(&mut self) {
        if let Some(row) = self.target_rows.get_mut(self.target_cursor) {
            row.selected = !row.selected;
        }
    }

    fn cycle_clipboard_format(&mut self) {
        use ClipboardFormat::*;
        self.clipboard_format = match self.clipboard_format {
            RemotePath => ScpStyle,
            ScpStyle => SshPath,
            SshPath => Custom,
            Custom => RemotePath,
        };
        // Mirror into cfg so persist_config picks it up — the TUI tracks
        // clipboard_format separately for ergonomic reasons but the
        // on-disk source of truth is cfg.clipboard_format.
        self.cfg.clipboard_format = self.clipboard_format;
        self.toast(&format!("clipboard: {:?}", self.clipboard_format));
        self.persist_config();
    }

    fn selected_target_names(&self) -> Vec<String> {
        let mut out = vec![];
        for row in &self.target_rows {
            if !row.selected {
                continue;
            }
            match row.kind {
                TargetKind::Single => out.push(row.name.clone()),
                TargetKind::Group => {
                    if let Some(g) = self.cfg.group_by_name(&row.name) {
                        for t in &g.targets {
                            if !out.contains(t) {
                                out.push(t.clone());
                            }
                        }
                    }
                }
            }
        }
        out
    }

    fn start_queue_sync(&mut self) {
        if self.queue.is_empty() {
            self.toast("queue is empty");
            return;
        }
        let target_names = self.selected_target_names();
        if target_names.is_empty() {
            self.toast("no target selected");
            return;
        }
        // Filter out targets whose latest preflight said they're not OK; this
        // protects the Auto mode flow from silently enqueueing failures.
        // NoRsync is kept — the transfer will fail fast on the remote side and
        // user can react, rather than having us silently drop it.
        let (ok, skipped): (Vec<String>, Vec<String>) = target_names.into_iter().partition(|tn| {
            self.target_rows
                .iter()
                .find(|r| r.name == *tn)
                .map(|r| !matches!(r.status, TargetStatus::Unreachable(_)))
                .unwrap_or(true)
        });
        if ok.is_empty() {
            self.toast(&format!(
                "all selected targets unreachable ({})",
                skipped.join(", ")
            ));
            return;
        }
        let files: Vec<PathBuf> = std::mem::take(&mut self.queue);
        self.queue_cursor = 0;

        for file in &files {
            for tname in &ok {
                let Some(t) = self.cfg.target_by_name(tname) else {
                    continue;
                };
                let id = self.next_transfer_id;
                self.next_transfer_id += 1;
                let tr = Transfer::new(id, t, file.clone());
                self.transfer_index.insert(id, self.transfers.len());
                self.transfers.push(tr);
                transfer::spawn(id, t.clone(), file.clone(), self.transfer_tx.clone());
            }
        }
        if skipped.is_empty() {
            self.toast(&format!(
                "sync {} file(s) → {} target(s)",
                files.len(),
                ok.len()
            ));
        } else {
            self.toast(&format!(
                "sync {} file(s) → {} (skipped {})",
                files.len(),
                ok.join(", "),
                skipped.join(", ")
            ));
        }
    }

    fn handle_transfer_event(&mut self, ev: TransferEvent) {
        match ev {
            TransferEvent::Started { id, .. } => {
                if let Some(t) = self.transfer_mut(id) {
                    t.state = TransferState::Running;
                }
            }
            TransferEvent::Progress {
                id,
                percent,
                rate,
                bytes: _,
            } => {
                if let Some(t) = self.transfer_mut(id) {
                    t.percent = percent;
                    t.rate = rate;
                }
            }
            TransferEvent::Line { .. } => {}
            TransferEvent::Completed { id, remote_abs_dir } => {
                let (local, target_name) = match self.transfer_mut(id) {
                    Some(t) => {
                        t.state = TransferState::Completed;
                        t.percent = 100;
                        t.remote_abs_dir = remote_abs_dir.clone();
                        (t.local.clone(), t.target_name.clone())
                    }
                    None => return,
                };
                self.record_history(&target_name, &local, &remote_abs_dir);
                self.update_clipboard_for_completed(&target_name, &local, &remote_abs_dir);
            }
            TransferEvent::Failed { id, error } => {
                if let Some(t) = self.transfer_mut(id) {
                    t.state = TransferState::Failed;
                    t.last_error = Some(error.clone());
                }
                self.toast(&format!("transfer failed: {error}"));
            }
        }
    }

    fn transfer_mut(&mut self, id: u64) -> Option<&mut Transfer> {
        let idx = *self.transfer_index.get(&id)?;
        self.transfers.get_mut(idx)
    }

    fn record_history(&mut self, target_name: &str, local: &std::path::Path, remote_abs_dir: &str) {
        let Some(target) = self.cfg.target_by_name(target_name) else {
            return;
        };
        let local_abs = std::fs::canonicalize(local)
            .unwrap_or_else(|_| local.to_path_buf())
            .to_string_lossy()
            .into_owned();
        let local_name = local
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let remote_path = format!("{}/{}", remote_abs_dir.trim_end_matches('/'), local_name);
        let target_host = match &target.user {
            Some(u) => format!("{u}@{}", target.host),
            None => target.host.clone(),
        };
        let size = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
        let entry = HistoryEntry {
            local_path: local_abs,
            local_name,
            size,
            target_name: target_name.to_string(),
            target_host,
            remote_path,
            synced_at: crate::history::now_epoch(),
            status: HistoryStatus::Completed,
        };
        if let Err(e) = self.history.append(entry) {
            self.toast(&format!("history append failed: {e}"));
        }
    }

    fn recopy_completed(&mut self, id: u64) {
        let Some(idx) = self.transfer_index.get(&id).copied() else {
            return;
        };
        let Some(t) = self.transfers.get(idx) else {
            return;
        };
        if t.state != TransferState::Completed {
            self.toast("transfer not completed yet");
            return;
        }
        let local = t.local.clone();
        let target_name = t.target_name.clone();
        let remote = t.remote_abs_dir.clone();
        self.update_clipboard_for_completed(&target_name, &local, &remote);
    }

    fn update_clipboard_for_completed(
        &mut self,
        target_name: &str,
        local: &std::path::Path,
        remote_abs_dir: &str,
    ) {
        let Some(t) = self.cfg.target_by_name(target_name) else {
            return;
        };
        let text = crate::clipboard::render(
            t,
            local,
            remote_abs_dir,
            self.clipboard_format,
            &self.cfg.clipboard_template,
        );
        let remote_path = format!(
            "{}/{}",
            remote_abs_dir.trim_end_matches('/'),
            local
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        self.write_clipboard_text(text, Some(remote_path));
    }

    fn toast(&mut self, msg: &str) {
        self.toast = Some((msg.to_string(), std::time::Instant::now()));
    }
}

fn trunc(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (count, c) in s.chars().enumerate() {
        if count >= max {
            out.push('…');
            return out;
        }
        out.push(c);
    }
    out
}
