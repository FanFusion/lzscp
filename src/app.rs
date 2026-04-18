use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::path_input;
use crate::target::{ClipboardFormat, SyncMode};
use crate::transfer::{self, Transfer, TransferEvent, TransferState};

#[derive(Debug, Clone)]
pub enum AppEvent {
    Terminal(Event),
    TransferUpdate(TransferEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    DropZone,
    Targets,
    Progress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Single,
    Group,
}

#[derive(Debug, Clone)]
pub struct TargetRow {
    pub name: String,
    pub kind: TargetKind,
    pub summary: String,
    pub selected: bool,
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

    pub transfer_tx: mpsc::UnboundedSender<TransferEvent>,
    pub transfer_rx: mpsc::UnboundedReceiver<TransferEvent>,
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
            })
            .collect();
        for g in &cfg.groups {
            rows.push(TargetRow {
                name: g.name.clone(),
                kind: TargetKind::Group,
                summary: format!("group → {}", g.targets.join(" + ")),
                selected: false,
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
            transfer_tx: tx,
            transfer_rx: rx,
        }
    }

    pub fn tick(&mut self) {
        if let Some((_, at)) = &self.toast {
            if at.elapsed() > std::time::Duration::from_secs(4) {
                self.toast = None;
            }
        }
    }

    pub fn handle_event(&mut self, evt: AppEvent) {
        match evt {
            AppEvent::Terminal(Event::Key(k)) => self.handle_key(k),
            AppEvent::Terminal(Event::Paste(s)) => self.handle_paste(&s),
            AppEvent::Terminal(Event::Mouse(m)) => match m.kind {
                MouseEventKind::ScrollDown => self.move_cursor(1),
                MouseEventKind::ScrollUp => self.move_cursor(-1),
                _ => {}
            },
            AppEvent::Terminal(_) => {}
            AppEvent::TransferUpdate(u) => self.handle_transfer_event(u),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if self.help_visible {
            self.help_visible = false;
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('?') => self.help_visible = true,
            KeyCode::Tab => self.cycle_focus(1),
            KeyCode::BackTab => self.cycle_focus(-1),
            KeyCode::Char('a') => {
                self.mode = SyncMode::Auto;
                self.toast("mode: auto");
            }
            KeyCode::Char('m') => {
                self.mode = SyncMode::Manual;
                self.toast("mode: manual");
            }
            KeyCode::Char('c') => self.cycle_clipboard_format(),
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Char(' ') if self.focus == Focus::Targets => self.toggle_target_cursor(),
            KeyCode::Char(d @ '1'..='9') if self.focus != Focus::Progress => {
                let idx = (d as u8 - b'1') as usize;
                if idx < self.target_rows.len() {
                    self.target_cursor = idx;
                    self.toggle_target_cursor();
                }
            }
            KeyCode::Enter => self.start_queue_sync(),
            KeyCode::Delete | KeyCode::Backspace if self.focus == Focus::DropZone => {
                if !self.queue.is_empty() && self.queue_cursor < self.queue.len() {
                    self.queue.remove(self.queue_cursor);
                    if self.queue_cursor >= self.queue.len() && self.queue_cursor > 0 {
                        self.queue_cursor -= 1;
                    }
                }
            }
            KeyCode::Char('x') if self.focus == Focus::DropZone => {
                self.queue.clear();
                self.queue_cursor = 0;
            }
            _ => {}
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
            Focus::Progress => {}
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
        self.toast(&format!("clipboard: {:?}", self.clipboard_format));
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
        let files: Vec<PathBuf> = std::mem::take(&mut self.queue);
        self.queue_cursor = 0;

        for file in &files {
            for tname in &target_names {
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
        self.toast(&format!(
            "sync {} file(s) → {} target(s)",
            files.len(),
            target_names.len()
        ));
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
            TransferEvent::Completed {
                id,
                remote_abs_dir,
            } => {
                let (local, target_name) = match self.transfer_mut(id) {
                    Some(t) => {
                        t.state = TransferState::Completed;
                        t.percent = 100;
                        t.remote_abs_dir = remote_abs_dir.clone();
                        (t.local.clone(), t.target_name.clone())
                    }
                    None => return,
                };
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
        match crate::clipboard::write(&text) {
            Ok(()) => {
                self.last_clipboard = Some(text.clone());
                self.toast(&format!("clipboard ← {}", trunc(&text, 60)));
            }
            Err(e) => {
                self.toast(&format!("clipboard error: {e}"));
            }
        }
    }

    fn toast(&mut self, msg: &str) {
        self.toast = Some((msg.to_string(), std::time::Instant::now()));
    }
}

fn trunc(s: &str, max: usize) -> String {
    let mut it = s.chars();
    let mut out = String::new();
    let mut count = 0;
    for c in it.by_ref() {
        if count >= max {
            out.push('…');
            return out;
        }
        out.push(c);
        count += 1;
    }
    out
}
