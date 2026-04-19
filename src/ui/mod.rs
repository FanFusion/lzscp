pub mod theme;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{
    ActivityRef, App, Focus, HelpBarAction, HitRegions, ModalHit, Tab, TargetKind, TargetStatus,
    UpdateStatus, WatchUi, WatchUiStatus,
};
use crate::history::HistoryEntry;
use crate::target::SyncMode;
use crate::transfer::{Transfer, TransferState};

pub fn draw(f: &mut Frame<'_>, app: &mut App) {
    let palette = theme::palette(&app.cfg.theme);
    let size = f.area();

    // Reset hit-test regions for this frame.
    app.hit_regions = HitRegions::default();

    // Every tab shares the two-line header (title + tab bar), a toast line
    // and the help bar. Only the body in between differs.
    let body_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(1), // tab bar
            Constraint::Min(6),    // body (tab-specific)
            Constraint::Length(1), // toast
            Constraint::Length(1), // help bar
        ])
        .split(size);

    draw_title(f, body_chunks[0], app, &palette);
    draw_tab_bar(f, body_chunks[1], app, &palette);

    match app.current_tab {
        Tab::Drop => draw_drop_tab_body(f, body_chunks[2], app, &palette),
        Tab::Watch => draw_watch_tab_body(f, body_chunks[2], app, &palette),
    }

    draw_toast(f, body_chunks[3], app, &palette);
    draw_help_bar(f, body_chunks[4], app, &palette);

    if app.help_visible {
        draw_help_overlay(f, size, &palette);
        app.hit_regions.modal_area = Some(size);
    }
    if !matches!(app.update_status, UpdateStatus::Idle) {
        draw_update_overlay(f, size, app, &palette);
    }
    if app.menu_visible {
        draw_menu_overlay(f, size, app, &palette);
    }
    if app.ssh_picker.is_some() {
        draw_ssh_picker(f, size, app, &palette);
    }
}

fn draw_tab_bar(f: &mut Frame<'_>, area: Rect, app: &App, p: &theme::Palette) {
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw(" "));
    for (i, tab) in [Tab::Drop, Tab::Watch].iter().enumerate() {
        let active = app.current_tab == *tab;
        let key = match tab {
            Tab::Drop => "1",
            Tab::Watch => "2",
        };
        let style = if active {
            Style::default()
                .bg(p.accent)
                .fg(p.bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.muted)
        };
        spans.push(Span::styled(format!(" {} {} ", key, tab.label()), style));
        if i == 0 {
            spans.push(Span::raw(" "));
        }
    }
    // Tail — show watch activity count when not on the Watch tab so the
    // user notices if the poller is firing in the background.
    let live_watches = app.watch_handles.len();
    if live_watches > 0 && app.current_tab != Tab::Watch {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(
            format!("● {live_watches} watching"),
            Style::default().fg(p.diff_add),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_drop_tab_body(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let drop_zone_h = 8.min(area.height.saturating_sub(2));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),              // targets + activity
            Constraint::Length(drop_zone_h), // drop zone
        ])
        .split(area);
    let tp_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    app.hit_regions.targets_panel = tp_row[0];
    app.hit_regions.progress_panel = tp_row[1];
    app.hit_regions.drop_zone = chunks[1];

    draw_targets(f, tp_row[0], app, p);
    draw_activity(f, tp_row[1], app, p);
    draw_drop_zone(f, chunks[1], app, p);
}

fn draw_watch_tab_body(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);
    draw_watches(f, chunks[0], app, p);
    draw_watch_recent(f, chunks[1], app, p);
}

fn draw_watches(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let focused = app.focus == Focus::Watches;
    let title = if focused { " Folders " } else { " folders " };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style(focused, p));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.watches.is_empty() {
        let hint = vec![
            Line::from(Span::styled(
                "  No watches configured.",
                Style::default().fg(p.muted),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Add a [[watch]] block to your config.toml — see examples/",
                Style::default().fg(p.muted),
            )),
            Line::from(""),
            Line::from(Span::styled("    [[watch]]", Style::default().fg(p.muted))),
            Line::from(Span::styled(
                "    name    = \"screenshots\"",
                Style::default().fg(p.muted),
            )),
            Line::from(Span::styled(
                "    path    = \"~/Desktop\"",
                Style::default().fg(p.muted),
            )),
            Line::from(Span::styled(
                "    targets = [\"dev\"]",
                Style::default().fg(p.muted),
            )),
        ];
        f.render_widget(Paragraph::new(hint).wrap(Wrap { trim: false }), inner);
        return;
    }

    // Clamp the cursor to the current list size.
    let max = app.watches.len().saturating_sub(1);
    if app.watch_cursor > max {
        app.watch_cursor = max;
    }
    let cursor = app.watch_cursor;
    let lines: Vec<Line<'_>> = app
        .watches
        .iter()
        .enumerate()
        .map(|(i, w)| render_watch_row(w, i == cursor && focused, p))
        .collect();

    // Manual paragraph render lets us do per-row cursor highlighting via bg.
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_watch_row<'a>(w: &'a WatchUi, is_cursor: bool, p: &theme::Palette) -> Line<'a> {
    let (glyph, glyph_style) = match &w.status {
        WatchUiStatus::Off => ("○", Style::default().fg(p.muted)),
        WatchUiStatus::Starting => ("●", Style::default().fg(p.accent)),
        WatchUiStatus::Running => (
            "●",
            Style::default().fg(p.diff_add).add_modifier(Modifier::BOLD),
        ),
        WatchUiStatus::LockedByOther(_) => ("⚠", Style::default().fg(p.diff_del)),
        WatchUiStatus::Error(_) => ("✗", Style::default().fg(p.diff_del)),
    };
    let name = pad_or_trunc(&w.name, 14);
    let path = pad_or_trunc(&w.path_display, 30);
    let targets = pad_or_trunc(&format!("→ {}", w.targets_display), 22);
    let base_tail = match &w.status {
        WatchUiStatus::Off => format!("{} synced", w.synced_count),
        WatchUiStatus::Starting => "starting…".to_string(),
        WatchUiStatus::Running => format!("{} synced", w.synced_count),
        WatchUiStatus::LockedByOther(pid) => format!("locked by PID {pid}"),
        WatchUiStatus::Error(msg) => truncate_cols(msg, 30),
    };
    let tail = if !w.pending_catchup.is_empty() {
        format!("{base_tail}  ({} new — press r)", w.pending_catchup.len())
    } else {
        base_tail
    };
    let tail_style = match &w.status {
        WatchUiStatus::Off => Style::default().fg(p.muted),
        WatchUiStatus::Starting => Style::default().fg(p.accent),
        WatchUiStatus::Running => Style::default().fg(p.diff_add),
        WatchUiStatus::LockedByOther(_) | WatchUiStatus::Error(_) => {
            Style::default().fg(p.diff_del)
        }
    };
    let base_style = if is_cursor {
        Style::default()
            .bg(p.selection)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.fg)
    };
    Line::from(vec![
        Span::styled(" ", base_style),
        Span::styled(glyph.to_string(), glyph_style.patch(base_style)),
        Span::styled(" ", base_style),
        Span::styled(name, base_style),
        Span::styled(" ", base_style),
        Span::styled(path, base_style.patch(Style::default().fg(p.muted))),
        Span::styled(" ", base_style),
        Span::styled(targets, base_style.patch(Style::default().fg(p.accent))),
        Span::styled(" ", base_style),
        Span::styled(tail, base_style.patch(tail_style)),
    ])
}

fn draw_watch_recent(f: &mut Frame<'_>, area: Rect, app: &App, p: &theme::Palette) {
    let block = Block::default()
        .title(" recent ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.muted));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.watch_recent.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            "  (no files synced yet — enable a watch above with Space)",
            Style::default().fg(p.muted),
        )));
        f.render_widget(msg, inner);
        return;
    }
    let lines: Vec<Line<'_>> = app
        .watch_recent
        .iter()
        .take(inner.height as usize)
        .map(|e| {
            let ago = humanize_elapsed(e.at);
            Line::from(vec![
                Span::raw(" "),
                Span::styled("📸", Style::default().fg(p.accent)),
                Span::raw(" "),
                Span::styled(pad_or_trunc(&e.file_display, 28), Style::default().fg(p.fg)),
                Span::raw(" "),
                Span::styled(format!("→ {}", e.watch_name), Style::default().fg(p.accent)),
                Span::raw("  "),
                Span::styled(ago, Style::default().fg(p.muted)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn humanize_elapsed(t: std::time::Instant) -> String {
    let d = t.elapsed();
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

fn draw_ssh_picker(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let Some(picker) = app.ssh_picker.clone() else {
        return;
    };
    let list_len = picker.list.len();
    let h = ((list_len as u16) + 4)
        .min(area.height.saturating_sub(4))
        .max(6);
    let w = 72.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);
    app.hit_regions.modal_area = Some(rect);
    app.hit_regions.ssh_picker_rows.clear();

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    let max_rows = h.saturating_sub(3) as usize;
    let inner_w = w.saturating_sub(2) as usize;
    for (i, host) in picker.list.iter().enumerate().take(max_rows) {
        let selected = i == picker.cursor;
        let row_y = rect.y + 1 + (i as u16) + 1;
        let endpoint = format!(
            "{}{}{}",
            host.user.as_deref().unwrap_or(""),
            if host.user.is_some() { "@" } else { "" },
            host.hostname.clone().unwrap_or_else(|| host.name.clone()),
        );
        // Render the row as a single pre-padded string so we can apply one
        // consistent highlight style across the whole line — otherwise a
        // partial-width highlight (label only) hides the endpoint when the
        // unstyled background matches the selected fg.
        let raw = format!("  {}. {}  {}", i + 1, host.name, endpoint);
        let padded = if raw.chars().count() < inner_w {
            format!("{}{}", raw, " ".repeat(inner_w - raw.chars().count()))
        } else {
            raw
        };
        let style = if selected {
            Style::default()
                .fg(p.bg)
                .bg(p.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg)
        };
        lines.push(Line::from(Span::styled(padded, style)));
        app.hit_regions
            .ssh_picker_rows
            .push(Rect::new(rect.x + 1, row_y, w.saturating_sub(2), 1));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter to add · Esc to cancel",
        Style::default().fg(p.muted),
    )));

    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Add target from ~/.ssh/config ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(p.bg).fg(p.fg));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_toast(f: &mut Frame<'_>, area: Rect, app: &App, p: &theme::Palette) {
    let line = match &app.toast {
        Some((s, _)) => Line::from(Span::styled(
            format!(" • {s}"),
            Style::default().fg(p.diff_add),
        )),
        None => Line::from(""),
    };
    f.render_widget(Paragraph::new(line).style(Style::default().bg(p.bg)), area);
}

fn draw_menu_overlay(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let actions = crate::app::MenuAction::all();
    let h = (actions.len() as u16 + 4).min(area.height.saturating_sub(4));
    let w = 48.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);
    app.hit_regions.modal_area = Some(rect);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    for (i, action) in actions.iter().enumerate() {
        let selected = i == app.menu_cursor;
        let row_y = rect.y + 1 + (i as u16) + 1;
        let label_span = Span::styled(
            format!("  {}. {}", i + 1, action.label()),
            if selected {
                Style::default()
                    .fg(p.bg)
                    .bg(p.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(p.fg)
            },
        );
        let kb = Span::styled(
            format!("  [{}]", action.shortcut()),
            Style::default().fg(if selected { p.bg } else { p.muted }),
        );
        let mut row_spans = vec![label_span];
        let pad_w = w.saturating_sub(2) as i32
            - (2 + 3
                + action.label().chars().count() as i32
                + 4
                + action.shortcut().chars().count() as i32);
        if pad_w > 0 {
            row_spans.push(Span::raw(" ".repeat(pad_w as usize)));
        }
        row_spans.push(kb);
        lines.push(Line::from(row_spans));

        app.hit_regions.menu_rows.push((
            Rect::new(rect.x + 1, row_y, w.saturating_sub(2), 1),
            *action,
        ));
    }

    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Menu  (Ctrl+P) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(p.bg).fg(p.fg));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_update_overlay(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let w = 60.min(area.width.saturating_sub(4));
    let h = 10.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);
    app.hit_regions.modal_area = Some(rect);

    let (title, lines): (&str, Vec<Line>) = match &app.update_status {
        UpdateStatus::Idle => (" Update ", vec![]),
        UpdateStatus::Checking => (
            " Update ",
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Checking for updates…",
                    Style::default().fg(p.fg),
                )),
            ],
        ),
        UpdateStatus::Available(v) => (
            " Update available ",
            vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("  current: ", Style::default().fg(p.muted)),
                    Span::styled(format!("v{}", crate::VERSION), Style::default().fg(p.fg)),
                ]),
                Line::from(vec![
                    Span::styled("  latest:  ", Style::default().fg(p.muted)),
                    Span::styled(
                        format!("v{v}"),
                        Style::default().fg(p.diff_add).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "  Download and install now?",
                    Style::default().fg(p.fg),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        " y ",
                        Style::default()
                            .fg(p.bg)
                            .bg(p.diff_add)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  confirm     ", Style::default().fg(p.muted)),
                    Span::styled(
                        " n / Esc ",
                        Style::default()
                            .fg(p.bg)
                            .bg(p.muted)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  cancel", Style::default().fg(p.muted)),
                ]),
            ],
        ),
        UpdateStatus::Installing(v) => (
            " Updating ",
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  Downloading v{v}…"),
                    Style::default().fg(p.fg),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  (~5 MB binary; may take a while on slow links)",
                    Style::default().fg(p.muted),
                )),
            ],
        ),
        UpdateStatus::Installed(paths) => {
            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Update installed.",
                    Style::default().fg(p.diff_add).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for p_ in paths {
                lines.push(Line::from(Span::styled(
                    format!("    {}", p_.display()),
                    Style::default().fg(p.muted),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Restart lzsync to use the new version.",
                Style::default().fg(p.fg),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  (press any key to dismiss)",
                Style::default().fg(p.muted),
            )));
            (" Update complete ", lines)
        }
        UpdateStatus::Failed(e) => (
            " Update failed ",
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Could not update:",
                    Style::default().fg(p.diff_del).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(format!("  {e}"), Style::default().fg(p.fg))),
                Line::from(""),
                Line::from(Span::styled(
                    "  (press any key to dismiss)",
                    Style::default().fg(p.muted),
                )),
            ],
        ),
    };

    let border_color = match &app.update_status {
        UpdateStatus::Available(_) => p.diff_add,
        UpdateStatus::Failed(_) => p.diff_del,
        _ => p.accent,
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().bg(p.bg).fg(p.fg));
    f.render_widget(Paragraph::new(lines).block(block), rect);

    // Click zones.
    match &app.update_status {
        UpdateStatus::Available(_) => {
            // The button row is the last visible line (second to last before
            // border). In our layout that's rect.y + 8 (border + 7 lines of
            // content; button row is the 7th content line → index 7).
            let btn_row = rect.y + 8;
            if btn_row < rect.y + rect.height.saturating_sub(1) {
                let y_btn = Rect::new(rect.x + 2, btn_row, 4, 1);
                let n_btn = Rect::new(rect.x + 21, btn_row, 9, 1);
                app.hit_regions.modal_hits.push((y_btn, ModalHit::Confirm));
                app.hit_regions.modal_hits.push((n_btn, ModalHit::Cancel));
            }
        }
        UpdateStatus::Installed(_) | UpdateStatus::Failed(_) => {
            // Whole modal body dismisses.
            app.hit_regions.modal_hits.push((rect, ModalHit::Dismiss));
        }
        _ => {}
    }
}

fn draw_title(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let mode = match app.mode {
        SyncMode::Auto => "auto",
        SyncMode::Manual => "manual",
    };
    let title = Line::from(vec![
        Span::styled(
            " lzsync ",
            Style::default()
                .bg(p.accent)
                .fg(p.bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(format!("v{}", crate::VERSION), Style::default().fg(p.muted)),
        Span::raw("   "),
        Span::styled(format!("[{mode}]"), Style::default().fg(p.accent)),
        Span::raw("   "),
        Span::styled(
            format!("clipboard: {:?}", app.clipboard_format),
            Style::default().fg(p.muted),
        ),
    ]);
    f.render_widget(
        Paragraph::new(title).style(Style::default().fg(p.fg).bg(p.bg)),
        area,
    );
}

fn draw_drop_zone(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let focused = app.focus == Focus::DropZone;
    let title = if app.queue.is_empty() {
        " Drop files / paste paths "
    } else {
        " Queue "
    };

    let block = Block::default()
        .title(Line::from(title))
        .borders(Borders::ALL)
        .border_style(border_style(focused, p))
        .style(Style::default().fg(p.fg).bg(p.bg));

    if app.queue.is_empty() {
        let inner_h = area.height.saturating_sub(2);
        let selected_targets: Vec<&str> = app
            .target_rows
            .iter()
            .filter(|r| r.selected)
            .map(|r| r.name.as_str())
            .collect();

        // Reserve 2 rows for the "will send to…" footer so the hint still
        // centres visually.
        let content_lines = if selected_targets.is_empty() { 4 } else { 6 };
        let pad_top = (inner_h.saturating_sub(content_lines) / 2) as usize;

        let mut hint: Vec<Line> = Vec::new();
        for _ in 0..pad_top {
            hint.push(Line::from(""));
        }
        hint.push(center_line(
            area.width,
            vec![Span::styled(
                "⬇  Drop files here",
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            )],
        ));
        hint.push(center_line(
            area.width,
            vec![Span::styled(
                "or Cmd+V / Ctrl+Shift+V paste paths",
                Style::default().fg(p.fg),
            )],
        ));
        if !selected_targets.is_empty() {
            hint.push(Line::from(""));
            hint.push(center_line(
                area.width,
                vec![
                    Span::styled("→ will send to: ", Style::default().fg(p.muted)),
                    Span::styled(
                        selected_targets.join(", "),
                        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
                    ),
                ],
            ));
        } else {
            hint.push(Line::from(""));
            hint.push(center_line(
                area.width,
                vec![Span::styled(
                    "(select a target above first)",
                    Style::default().fg(p.diff_del),
                )],
            ));
        }
        if let Some(err) = &app.last_paste_error {
            hint.push(Line::from(""));
            hint.push(center_line(
                area.width,
                vec![Span::styled(
                    format!("⚠ {err}"),
                    Style::default().fg(p.diff_del),
                )],
            ));
        }
        f.render_widget(
            Paragraph::new(hint).block(block).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    // Record clickable rows (one row per queue item, starting below the top
    // border at area.y + 1).
    let inner_x = area.x + 1;
    let inner_w = area.width.saturating_sub(2);
    for (i, _) in app.queue.iter().enumerate() {
        let row_y = area.y + 1 + i as u16;
        if row_y >= area.y + area.height.saturating_sub(1) {
            break;
        }
        app.hit_regions
            .queue_rows
            .push(Rect::new(inner_x, row_y, inner_w, 1));
    }

    let items: Vec<ListItem> = app
        .queue
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let mut style = Style::default().fg(p.fg);
            if focused && i == app.queue_cursor {
                style = style.bg(p.selection).add_modifier(Modifier::BOLD);
            }
            let name = path.display().to_string();
            let size = fs_size_pretty(path);
            let line = Line::from(vec![
                Span::styled(format!(" {name}"), style),
                Span::raw("  "),
                Span::styled(size, Style::default().fg(p.muted)),
            ]);
            ListItem::new(line)
        })
        .collect();

    f.render_widget(List::new(items).block(block), area);
}

fn draw_targets(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let focused = app.focus == Focus::Targets;
    let block = Block::default()
        .title(Line::from(" Targets "))
        .borders(Borders::ALL)
        .border_style(border_style(focused, p))
        .style(Style::default().fg(p.fg).bg(p.bg));

    if app.target_rows.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No targets yet.",
                Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  1.  Press Ctrl+P (or click the menu chip below)",
                Style::default().fg(p.muted),
            )),
            Line::from(Span::styled(
                "  2.  Choose \"Add target from ~/.ssh/config\"",
                Style::default().fg(p.muted),
            )),
            Line::from(Span::styled(
                "  3.  Pick a host — that's it.",
                Style::default().fg(p.muted),
            )),
        ];
        f.render_widget(Paragraph::new(lines).block(block), area);
        return;
    }

    // Record clickable rows (each target occupies one line).
    let inner_x = area.x + 1;
    let inner_w = area.width.saturating_sub(2);
    for (i, _) in app.target_rows.iter().enumerate() {
        let row_y = area.y + 1 + i as u16;
        if row_y >= area.y + area.height.saturating_sub(1) {
            break;
        }
        app.hit_regions
            .target_rows
            .push(Rect::new(inner_x, row_y, inner_w, 1));
        let _ = i; // silence unused warning if row loop logic changes
    }

    let items: Vec<ListItem> = app
        .target_rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let sel_glyph = if row.selected { "●" } else { "○" };
            let mut style = Style::default().fg(p.fg);
            if focused && i == app.target_cursor {
                style = style.bg(p.selection).add_modifier(Modifier::BOLD);
            }
            let tag = match row.kind {
                TargetKind::Single => "",
                TargetKind::Group => " (group)",
            };
            let (status_icon, status_style, status_tail) = status_display(&row.status, p);

            let line = Line::from(vec![
                Span::styled(
                    format!(" {sel_glyph} "),
                    Style::default()
                        .fg(if row.selected { p.accent } else { p.muted })
                        .add_modifier(if row.selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(status_icon, status_style),
                Span::raw(" "),
                Span::styled(format!("{i}. ", i = i + 1), Style::default().fg(p.muted)),
                Span::styled(format!("{}{tag}", row.name), style),
                Span::raw("  "),
                Span::styled(row.summary.clone(), Style::default().fg(p.muted)),
                Span::raw(" "),
                Span::styled(status_tail, Style::default().fg(p.muted)),
            ]);
            ListItem::new(line)
        })
        .collect();

    f.render_widget(List::new(items).block(block), area);
}

/// Unified Activity panel: live transfers at the top, then history groups
/// (same remote_path collapsed into one row). Click or cursor+Enter copies
/// the row's remote path; the last-copied row keeps a `📋 on clipboard`
/// badge so the user always knows what's in the paste buffer.
fn draw_activity(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let focused = app.focus == Focus::Progress;
    let filter_active = app.activity_filter.is_some();
    let query = app.activity_query().to_lowercase();

    // Live = anything not yet recorded in history (record_history only runs
    // on Completed, so Completed rows migrate to history automatically).
    let live: Vec<Transfer> = app
        .transfers
        .iter()
        .rev()
        .filter(|t| t.state != TransferState::Completed)
        .filter(|t| {
            if query.is_empty() {
                return true;
            }
            let fname = t
                .local
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            fname.to_lowercase().contains(&query) || t.target_name.to_lowercase().contains(&query)
        })
        .cloned()
        .collect();

    // History: filter, then group by remote_path (same file on 2 hosts = 1
    // row with both target names joined). Preserve newest-first order.
    let raw: Vec<HistoryEntry> = app
        .history
        .filter(&app.activity_query())
        .into_iter()
        .cloned()
        .collect();
    let groups: Vec<HistoryGroup> = group_history(&raw);

    let title = if filter_active {
        Line::from(vec![
            Span::raw(" Activity "),
            Span::styled(
                format!("· {} shown ", live.len() + groups.len()),
                Style::default().fg(p.muted),
            ),
        ])
    } else if app.history.entries.is_empty() && live.is_empty() {
        Line::from(" Activity ")
    } else {
        Line::from(vec![
            Span::raw(" Activity "),
            Span::styled(
                format!(
                    "· {} live · {} in history ",
                    live.len(),
                    app.history.entries.len()
                ),
                Style::default().fg(p.muted),
            ),
        ])
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style(focused, p))
        .style(Style::default().fg(p.fg).bg(p.bg));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width < 16 || inner.height == 0 {
        return;
    }
    let inner_w = inner.width as usize;
    let mut y_off: u16 = 0;

    // Filter bar (only when active).
    if filter_active {
        let q = app.activity_query();
        let line = Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "/",
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(q, Style::default().fg(p.fg)),
            Span::styled("▏", Style::default().fg(p.accent)),
            Span::raw("  "),
            Span::styled(
                "Esc to close",
                Style::default().fg(p.muted).add_modifier(Modifier::DIM),
            ),
        ]);
        f.render_widget(
            Paragraph::new(line).style(Style::default().bg(p.bg)),
            Rect::new(inner.x, inner.y + y_off, inner.width, 1),
        );
        y_off += 1;
    }

    if live.is_empty() && groups.is_empty() {
        if y_off < inner.height {
            let hint = if filter_active {
                "  no matches"
            } else {
                "  drop a file to start — history will appear here"
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(p.muted))))
                    .style(Style::default().bg(p.bg)),
                Rect::new(inner.x, inner.y + y_off, inner.width, 1),
            );
        }
        return;
    }

    // Clamp cursor and precompute "last copied" marker.
    let total_rows = live.len() + groups.len();
    if app.activity_cursor >= total_rows {
        app.activity_cursor = total_rows.saturating_sub(1);
    }
    let copied_marker = app.last_copied_remote.clone();

    let mut cursor_idx: usize = 0;

    // Live section.
    for t in &live {
        if y_off >= inner.height {
            break;
        }
        let y = inner.y + y_off;
        let is_cursor = focused && cursor_idx == app.activity_cursor;
        let is_copied = copied_marker
            .as_ref()
            .map(|cp| live_remote_matches(t, cp))
            .unwrap_or(false);
        let line = render_live_line(t, inner_w, p, is_cursor, is_copied);
        let rect = Rect::new(inner.x, y, inner.width, 1);
        f.render_widget(Paragraph::new(line).style(Style::default().bg(p.bg)), rect);
        app.hit_regions
            .activity_rows
            .push((rect, cursor_idx, ActivityRef::Live(t.id)));
        cursor_idx += 1;
        y_off += 1;
    }

    // History groups.
    for g in &groups {
        if y_off >= inner.height {
            break;
        }
        let y = inner.y + y_off;
        let is_cursor = focused && cursor_idx == app.activity_cursor;
        let is_copied = copied_marker
            .as_ref()
            .map(|cp| cp == &g.primary.remote_path)
            .unwrap_or(false);
        let line = render_history_group_line(g, inner_w, p, is_cursor, is_copied);
        let rect = Rect::new(inner.x, y, inner.width, 1);
        f.render_widget(Paragraph::new(line).style(Style::default().bg(p.bg)), rect);
        app.hit_regions
            .activity_rows
            .push((rect, cursor_idx, ActivityRef::History(g.primary_idx)));
        cursor_idx += 1;
        y_off += 1;
    }
}

/// Merged history row. `primary` is the newest entry for this remote_path;
/// `targets` collects every target that's synced the same file to the same
/// absolute remote path (usually one or two, occasionally more).
struct HistoryGroup {
    primary: HistoryEntry,
    /// Index of `primary` in the filtered history list (what
    /// `App::history.filter(query)` returns). Matches `ActivityRef::History`.
    primary_idx: usize,
    targets: Vec<String>,
}

fn group_history(matches: &[HistoryEntry]) -> Vec<HistoryGroup> {
    use std::collections::HashMap;
    let mut map: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<HistoryGroup> = Vec::new();
    for (i, e) in matches.iter().enumerate() {
        match map.get(&e.remote_path).copied() {
            Some(gi) => {
                if !out[gi].targets.contains(&e.target_name) {
                    out[gi].targets.push(e.target_name.clone());
                }
            }
            None => {
                map.insert(e.remote_path.clone(), out.len());
                out.push(HistoryGroup {
                    primary: e.clone(),
                    primary_idx: i,
                    targets: vec![e.target_name.clone()],
                });
            }
        }
    }
    out
}

fn live_remote_matches(t: &Transfer, expected_remote: &str) -> bool {
    if t.remote_abs_dir.is_empty() {
        return false;
    }
    let name = match t.local.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => return false,
    };
    let joined = format!("{}/{}", t.remote_abs_dir.trim_end_matches('/'), name);
    joined == expected_remote
}

const TARGET_COL: usize = 14;
const LEFT_FIXED: usize = 1 /*sp*/ + 1 /*icon*/ + 2 /*sp*/ + TARGET_COL + 2; /*sp*/
const RIGHT_GUTTER: usize = 2;
const COPIED_BADGE: &str = " 📋 on clipboard";

fn render_live_line(
    t: &Transfer,
    inner_w: usize,
    p: &theme::Palette,
    is_cursor: bool,
    is_copied: bool,
) -> Line<'static> {
    let (icon, icon_color) = match t.state {
        TransferState::Running => ("●", p.accent),
        TransferState::Pending => ("·", p.muted),
        TransferState::Failed => ("✗", p.diff_del),
        TransferState::Completed => ("✓", p.diff_add),
    };
    let target = pad_or_trunc(&t.target_name, TARGET_COL);
    let right = match t.state {
        TransferState::Running => {
            if t.rate.is_empty() {
                format!("{:>3}%", t.percent.min(100))
            } else {
                format!("{:>3}%  {}", t.percent.min(100), t.rate)
            }
        }
        TransferState::Pending => "queued".to_string(),
        TransferState::Failed => t
            .last_error
            .as_deref()
            .map(|e| truncate_cols(e, 30))
            .unwrap_or_else(|| "failed".to_string()),
        TransferState::Completed => "100%".to_string(),
    };
    let right_style = match t.state {
        TransferState::Running => Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        TransferState::Pending => Style::default().fg(p.muted),
        TransferState::Failed => Style::default().fg(p.diff_del),
        TransferState::Completed => Style::default().fg(p.diff_add),
    };

    let filename = t
        .local
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let filename = if t.source_watch.is_some() {
        format!("📸 {filename}")
    } else {
        filename
    };
    build_row_line(
        icon,
        icon_color,
        target,
        filename,
        p.fg,
        right,
        right_style,
        inner_w,
        p,
        is_cursor,
        is_copied,
    )
}

fn render_history_group_line(
    g: &HistoryGroup,
    inner_w: usize,
    p: &theme::Palette,
    is_cursor: bool,
    is_copied: bool,
) -> Line<'static> {
    let target_label = if g.targets.len() == 1 {
        g.targets[0].clone()
    } else {
        let joined = g.targets.join(",");
        if display_width(&joined) <= TARGET_COL {
            joined
        } else {
            format!("{} hosts", g.targets.len())
        }
    };
    let target = pad_or_trunc(&target_label, TARGET_COL);
    let right = crate::history::format_time_ago(g.primary.synced_at);
    let filename = if g.primary.source_watch.is_some() {
        format!("📸 {}", g.primary.local_name)
    } else {
        g.primary.local_name.clone()
    };
    build_row_line(
        "✓",
        p.diff_add,
        target,
        filename,
        p.fg,
        right,
        Style::default().fg(p.muted),
        inner_w,
        p,
        is_cursor,
        is_copied,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_row_line(
    icon: &'static str,
    icon_color: ratatui::style::Color,
    target: String,
    filename: String,
    name_fg: ratatui::style::Color,
    right: String,
    right_style: Style,
    inner_w: usize,
    p: &theme::Palette,
    is_cursor: bool,
    is_copied: bool,
) -> Line<'static> {
    let badge_w = if is_copied {
        display_width(COPIED_BADGE)
    } else {
        0
    };
    let right_w = display_width(&right);
    let fname_budget = inner_w
        .saturating_sub(LEFT_FIXED)
        .saturating_sub(right_w + badge_w + RIGHT_GUTTER);
    let name = truncate_cols(&filename, fname_budget.max(3));
    let name_w = display_width(&name);
    let pad = inner_w
        .saturating_sub(LEFT_FIXED + name_w + right_w + badge_w + RIGHT_GUTTER)
        .max(1);

    // The cursor row is highlighted by flipping the whole row's background
    // to the selection colour and bolding the text, matching the Targets
    // panel's selected-row treatment.
    let (bg_style, name_style) = if is_cursor {
        let bg = Style::default().bg(p.selection);
        (
            bg,
            Style::default()
                .fg(name_fg)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default(), Style::default().fg(name_fg))
    };
    let target_style = Style::default()
        .fg(p.accent)
        .add_modifier(Modifier::BOLD)
        .patch(bg_style);
    let icon_style = Style::default()
        .fg(icon_color)
        .add_modifier(Modifier::BOLD)
        .patch(bg_style);
    let muted_bg = Style::default().patch(bg_style);
    let right_style_bg = right_style.patch(bg_style);

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(10);
    spans.push(Span::styled(" ", muted_bg));
    spans.push(Span::styled(icon.to_string(), icon_style));
    spans.push(Span::styled("  ", muted_bg));
    spans.push(Span::styled(target, target_style));
    spans.push(Span::styled("  ", muted_bg));
    spans.push(Span::styled(name, name_style));
    spans.push(Span::styled(" ".repeat(pad), muted_bg));
    spans.push(Span::styled(right, right_style_bg));
    if is_copied {
        spans.push(Span::styled(
            COPIED_BADGE,
            Style::default()
                .fg(p.accent)
                .add_modifier(Modifier::BOLD)
                .patch(bg_style),
        ));
    }
    spans.push(Span::styled("  ", muted_bg));
    Line::from(spans)
}

fn draw_help_bar(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    // (key-label, description, click action). ≡ opens the menu which contains
    // every less-common action, so we keep the bar short.
    let chips: [(&str, &str, HelpBarAction); 6] = [
        ("Tab", "focus", HelpBarAction::CycleFocus),
        ("Enter", "sync", HelpBarAction::Sync),
        ("^U", "update", HelpBarAction::CheckUpdate),
        ("^P", "menu", HelpBarAction::OpenMenu),
        ("^H", "help", HelpBarAction::ToggleHelp),
        ("^Q", "quit", HelpBarAction::Quit),
    ];

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(chips.len() * 3);
    let mut cursor_x = area.x;
    for (i, (key, desc, action)) in chips.iter().enumerate() {
        let key_text: String = format!(" {key} ");
        let desc_text: String = if i + 1 == chips.len() {
            desc.to_string()
        } else {
            format!("{desc}  ")
        };
        let key_w = key_text.chars().count() as u16;
        let desc_w = desc_text.chars().count() as u16;
        let total_w = key_w + desc_w;

        if cursor_x + total_w > area.x + area.width {
            break; // ran out of space; silently drop remaining chips
        }

        app.hit_regions
            .help_bar_hits
            .push((Rect::new(cursor_x, area.y, total_w, 1), *action));

        spans.push(Span::styled(
            key_text,
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(desc_text, Style::default().fg(p.muted)));
        cursor_x += total_w;
    }

    let line = Line::from(spans);
    f.render_widget(Paragraph::new(line).style(Style::default().bg(p.bg)), area);
}

fn draw_help_overlay(f: &mut Frame<'_>, area: Rect, p: &theme::Palette) {
    let w = 62.min(area.width.saturating_sub(4));
    let h = 20.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);

    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .style(Style::default().bg(p.bg).fg(p.fg));
    let lines = vec![
        Line::from("  Ctrl+1 / Ctrl+2    switch Drop / Watch tab"),
        Line::from("  Ctrl+P             open menu / command palette"),
        Line::from("  Ctrl+H             toggle this help"),
        Line::from("  Ctrl+Q / Ctrl+C    quit"),
        Line::from("  Ctrl+U             check for update"),
        Line::from("  Ctrl+T             cycle theme"),
        Line::from("  Ctrl+A / Ctrl+N    auto / manual mode"),
        Line::from("  Ctrl+F             cycle clipboard format (or filter in Activity)"),
        Line::from(""),
        Line::from("  Tab / Shift+Tab    cycle focus within tab"),
        Line::from("  Up / Down / j / k  move cursor"),
        Line::from("  Enter              sync (DropZone) or copy (Activity)"),
        Line::from("  Space              toggle target / watch folder"),
        Line::from("  Backspace          remove queued file under cursor"),
        Line::from("  click a row        copy that remote path to clipboard"),
        Line::from(""),
        Line::from(Span::styled(
            "  All letter shortcuts use Ctrl so nothing in a dragged",
            Style::default().fg(p.muted),
        )),
        Line::from(Span::styled(
            "  path can trigger them by accident.",
            Style::default().fg(p.muted),
        )),
    ];
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn border_style(focused: bool, p: &theme::Palette) -> Style {
    if focused {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.muted)
    }
}

fn fs_size_pretty(p: &std::path::Path) -> String {
    let meta = match std::fs::metadata(p) {
        Ok(m) => m,
        Err(_) => return "?".into(),
    };
    let bytes = meta.len();
    const K: u64 = 1024;
    if bytes < K {
        format!("{bytes} B")
    } else if bytes < K * K {
        format!("{:.1} KB", bytes as f64 / K as f64)
    } else if bytes < K * K * K {
        format!("{:.1} MB", bytes as f64 / (K * K) as f64)
    } else {
        format!("{:.2} GB", bytes as f64 / (K * K * K) as f64)
    }
}

fn status_display(status: &TargetStatus, p: &theme::Palette) -> (String, Style, String) {
    match status {
        TargetStatus::Unknown => (
            "[?]".to_string(),
            Style::default().fg(p.muted),
            String::new(),
        ),
        TargetStatus::Probing => (
            "[…]".to_string(),
            Style::default().fg(p.accent),
            "probing".to_string(),
        ),
        TargetStatus::Reachable => (
            "[✓]".to_string(),
            Style::default().fg(p.diff_add),
            String::new(),
        ),
        TargetStatus::NoRsync => (
            "[⚠]".to_string(),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            "no rsync — click to install".to_string(),
        ),
        TargetStatus::Unreachable(e) => (
            "[✗]".to_string(),
            Style::default().fg(p.diff_del),
            summarize_ssh_error(e),
        ),
    }
}

fn summarize_ssh_error(e: &str) -> String {
    let first = e.lines().next().unwrap_or("").trim();
    // Collapse common failure modes into short human labels.
    if first.contains("Invalid command") {
        return "not a shell host (ssh rejects cmd)".to_string();
    }
    if first.contains("Connection closed") {
        return "connection closed".to_string();
    }
    if first.contains("Connection timed out") || first.contains("ConnectTimeout") {
        return "timed out".to_string();
    }
    if first.contains("Permission denied") {
        return "auth denied — add key to ssh-agent".to_string();
    }
    if first.contains("Could not resolve hostname") {
        return "hostname not resolvable".to_string();
    }
    if first.contains("Host key verification failed") {
        return "host key failed — accept in ssh first".to_string();
    }
    if first.contains("No route to host") {
        return "no route to host".to_string();
    }
    // Fall back to the first ~48 chars.
    let mut cut: String = first.chars().take(48).collect();
    if first.chars().count() > 48 {
        cut.push('…');
    }
    cut
}

fn center_line<'a>(width: u16, inner: Vec<Span<'a>>) -> Line<'a> {
    let content_w: usize = inner
        .iter()
        .map(|s| unicode_width_str(s.content.as_ref()))
        .sum();
    let inner_area_w = width.saturating_sub(2) as usize; // account for borders
    let left = inner_area_w.saturating_sub(content_w) / 2;
    let mut spans: Vec<Span<'a>> = Vec::with_capacity(inner.len() + 1);
    if left > 0 {
        spans.push(Span::raw(" ".repeat(left)));
    }
    spans.extend(inner);
    Line::from(spans)
}

fn unicode_width_str(s: &str) -> usize {
    display_width(s)
}

/// Terminal display width (CJK = 2 cols, emoji = 2 cols).
fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    s.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// Truncate by display columns with a middle ellipsis so filenames never
/// overflow a column budget — a char-count-based truncation would be wrong
/// for CJK since each char takes 2 terminal cols.
fn truncate_cols(s: &str, max_cols: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if max_cols == 0 {
        return String::new();
    }
    if display_width(s) <= max_cols {
        return s.to_string();
    }
    if max_cols == 1 {
        return "…".to_string();
    }
    let budget = max_cols - 1; // reserve 1 col for the ellipsis
    let prefix_budget = budget.div_ceil(2);
    let suffix_budget = budget - prefix_budget;

    let mut prefix = String::new();
    let mut pw = 0usize;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if pw + cw > prefix_budget {
            break;
        }
        prefix.push(c);
        pw += cw;
    }
    let mut suffix_rev: Vec<char> = Vec::new();
    let mut sw = 0usize;
    for c in s.chars().rev() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if sw + cw > suffix_budget {
            break;
        }
        suffix_rev.push(c);
        sw += cw;
    }
    let suffix: String = suffix_rev.into_iter().rev().collect();
    format!("{prefix}…{suffix}")
}

/// Pad with spaces (or middle-truncate) so the result is exactly `cols`
/// display columns wide.
fn pad_or_trunc(s: &str, cols: usize) -> String {
    let mut out = if display_width(s) > cols {
        truncate_cols(s, cols)
    } else {
        s.to_string()
    };
    let w = display_width(&out);
    if w < cols {
        out.push_str(&" ".repeat(cols - w));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_width_counts_cjk_and_emoji_as_two() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("截图"), 4);
        assert_eq!(display_width("a截b"), 4);
    }

    #[test]
    fn truncate_cols_short_circuits_when_fits() {
        assert_eq!(truncate_cols("hello", 10), "hello");
        assert_eq!(truncate_cols("", 10), "");
    }

    #[test]
    fn truncate_cols_never_overflows_budget() {
        for s in [
            "verylongfilenamewithoutanyspaces.png",
            "截图2026-04-19_11.31.26.png",
            "FireShot Capture 001 - Polymarket - [gemini.google.com].pdf",
            "🎉🎉🎉🎉🎉🎉.png",
        ] {
            for budget in [5, 10, 20, 40] {
                let t = truncate_cols(s, budget);
                assert!(
                    display_width(&t) <= budget,
                    "{s:?} trunc to {budget} → {t:?} (width {})",
                    display_width(&t)
                );
            }
        }
    }

    #[test]
    fn pad_or_trunc_is_exactly_cols_wide() {
        for (s, cols) in [
            ("dev", 10usize),
            ("devjp", 10),
            ("verylongtargetnamethatoverflows", 10),
            ("截图", 10),
        ] {
            assert_eq!(display_width(&pad_or_trunc(s, cols)), cols);
        }
    }
}
