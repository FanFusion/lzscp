pub mod theme;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Focus, HelpBarAction, HitRegions, ModalHit, TargetKind, UpdateStatus};
use crate::target::SyncMode;
use crate::transfer::TransferState;

pub fn draw(f: &mut Frame<'_>, app: &mut App) {
    let palette = theme::palette(&app.cfg.theme);
    let size = f.area();

    // Reset hit-test regions for this frame.
    app.hit_regions = HitRegions::default();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Length(7), // drop zone
            Constraint::Min(6),    // targets
            Constraint::Length(8), // progress
            Constraint::Length(2), // clipboard + toast
            Constraint::Length(1), // help
        ])
        .split(size);

    app.hit_regions.drop_zone = chunks[1];
    app.hit_regions.targets_panel = chunks[2];
    app.hit_regions.progress_panel = chunks[3];

    draw_title(f, chunks[0], app, &palette);
    draw_drop_zone(f, chunks[1], app, &palette);
    draw_targets(f, chunks[2], app, &palette);
    draw_progress(f, chunks[3], app, &palette);
    draw_status(f, chunks[4], app, &palette);
    draw_help_bar(f, chunks[5], app, &palette);

    if app.help_visible {
        draw_help_overlay(f, size, &palette);
        // Help overlay consumes its own rect for click dismissal.
        app.hit_regions.modal_area = Some(size);
    }
    if !matches!(app.update_status, UpdateStatus::Idle) {
        draw_update_overlay(f, size, app, &palette);
    }
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
                "  Restart lzscp to use the new version.",
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
            " lzscp ",
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
        let hint = vec![
            Line::from(Span::styled(
                "  ⬇  拖入文件 / Cmd+V 粘贴路径  (支持空格和中文)",
                Style::default().fg(p.muted),
            )),
            Line::from(""),
            Line::from(Span::styled(
                app.last_paste_error
                    .as_deref()
                    .map(|e| format!("  ⚠ {e}"))
                    .unwrap_or_default(),
                Style::default().fg(p.diff_del),
            )),
        ];
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
        let p1 = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No targets configured.",
                Style::default().fg(p.muted),
            )),
            Line::from(Span::styled(
                "  Create .lzscp/config.toml or ~/.config/lzscp/config.toml",
                Style::default().fg(p.muted),
            )),
        ])
        .block(block);
        f.render_widget(p1, area);
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
            let checkbox = if row.selected { "[✓]" } else { "[ ]" };
            let mut style = Style::default().fg(p.fg);
            if focused && i == app.target_cursor {
                style = style.bg(p.selection).add_modifier(Modifier::BOLD);
            }
            let tag = match row.kind {
                TargetKind::Single => "",
                TargetKind::Group => " (group)",
            };
            let line = Line::from(vec![
                Span::styled(
                    format!(" {checkbox} "),
                    Style::default().fg(if row.selected { p.accent } else { p.muted }),
                ),
                Span::styled(format!("{i}.  "), Style::default().fg(p.muted)),
                Span::styled(format!("{}{tag}", row.name), style),
                Span::raw("   "),
                Span::styled(row.summary.clone(), Style::default().fg(p.muted)),
            ]);
            ListItem::new(line)
        })
        .collect();

    f.render_widget(List::new(items).block(block), area);
}

fn draw_progress(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let focused = app.focus == Focus::Progress;
    let block = Block::default()
        .title(Line::from(" Progress "))
        .borders(Borders::ALL)
        .border_style(border_style(focused, p))
        .style(Style::default().fg(p.fg).bg(p.bg));

    if app.transfers.is_empty() {
        let hint = Paragraph::new(Line::from(Span::styled(
            "  (no transfers yet)",
            Style::default().fg(p.muted),
        )))
        .block(block);
        f.render_widget(hint, area);
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    let recent = app.transfers.iter().rev().take(4).collect::<Vec<_>>();
    let n = recent.len();
    let constraints: Vec<Constraint> = (0..n).map(|_| Constraint::Length(1)).collect();
    if constraints.is_empty() {
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (i, t) in recent.iter().enumerate() {
        let label = format!(
            "{}  {}  {}",
            t.target_name,
            t.local
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            t.rate
        );
        let color = match t.state {
            TransferState::Completed => p.diff_add,
            TransferState::Failed => p.diff_del,
            TransferState::Running => p.accent,
            TransferState::Pending => p.muted,
        };
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(color).bg(p.bg))
            .ratio(t.percent as f64 / 100.0)
            .label(Span::styled(
                label,
                Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
            ));
        f.render_widget(gauge, rows[i]);
    }
}

fn draw_status(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    let clipboard_line = match &app.last_clipboard {
        Some(v) => Line::from(vec![
            Span::styled(" clipboard: ", Style::default().fg(p.muted)),
            Span::styled(
                truncate_middle(v, area.width.saturating_sub(14) as usize),
                Style::default().fg(p.accent),
            ),
        ]),
        None => Line::from(Span::styled(
            " clipboard: (will populate after first successful transfer)",
            Style::default().fg(p.muted),
        )),
    };
    f.render_widget(
        Paragraph::new(clipboard_line).style(Style::default().bg(p.bg)),
        chunks[0],
    );

    let toast_line = match &app.toast {
        Some((s, _)) => Line::from(Span::styled(
            format!(" • {s}"),
            Style::default().fg(p.diff_add),
        )),
        None => Line::from(""),
    };
    f.render_widget(
        Paragraph::new(toast_line).style(Style::default().bg(p.bg)),
        chunks[1],
    );
}

fn draw_help_bar(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    // (key-label, description, click action)
    let chips: [(&str, &str, HelpBarAction); 8] = [
        ("Tab", "focus", HelpBarAction::CycleFocus),
        ("Space", "toggle", HelpBarAction::ToggleSelection),
        ("Enter", "sync", HelpBarAction::Sync),
        ("a/m", "mode", HelpBarAction::CycleMode),
        ("c", "clip-fmt", HelpBarAction::CycleClipboard),
        ("u", "update", HelpBarAction::CheckUpdate),
        ("?", "help", HelpBarAction::ToggleHelp),
        ("q", "quit", HelpBarAction::Quit),
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
    let w = 60.min(area.width.saturating_sub(4));
    let h = 18.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .style(Style::default().bg(p.bg).fg(p.fg));
    let lines = vec![
        Line::from("  Tab / Shift+Tab    cycle focus"),
        Line::from("  Up / Down          move cursor"),
        Line::from("  Space              toggle selected target"),
        Line::from("  1–9                quick-toggle Nth target"),
        Line::from("  Enter              execute sync (manual)"),
        Line::from("  a / m              auto / manual mode"),
        Line::from("  c                  cycle clipboard format"),
        Line::from("  u                  check for update"),
        Line::from("  Backspace          remove queued file under cursor"),
        Line::from("  x                  clear queue"),
        Line::from("  ?                  toggle this help"),
        Line::from("  q / Ctrl+C         quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  (drag files or paste paths into drop zone)",
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

fn truncate_middle(s: &str, max: usize) -> String {
    if max < 8 || s.chars().count() <= max {
        return s.to_string();
    }
    let half = (max - 1) / 2;
    let chars: Vec<char> = s.chars().collect();
    let total = chars.len();
    let head: String = chars.iter().take(half).collect();
    let tail: String = chars.iter().skip(total - half).collect();
    format!("{head}…{tail}")
}
