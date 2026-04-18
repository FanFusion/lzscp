pub mod theme;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{
    App, Focus, HelpBarAction, HitRegions, ModalHit, TargetKind, TargetStatus, UpdateStatus,
};
use crate::target::SyncMode;
use crate::transfer::TransferState;

pub fn draw(f: &mut Frame<'_>, app: &mut App) {
    let palette = theme::palette(&app.cfg.theme);
    let size = f.area();

    // Reset hit-test regions for this frame.
    app.hit_regions = HitRegions::default();

    // New ergonomic layout (Mac dock is at the bottom → drag motion goes
    // upward from the bottom edge of the terminal, so Drop zone lives there).
    //
    //   Title
    //   Targets │ Progress   (50/50 split, fixed 9 lines)
    //   Clipboard strip      (1 line — the primary output of the app)
    //   Drop zone            (flexible, takes remaining vertical space)
    //   Toast                (1 line)
    //   Help bar             (1 line)
    // Targets + progress share the top half dynamically; drop zone is a
    // fixed, smaller strip at the bottom so the rest of the UI gets room to
    // breathe on tall terminals.
    let drop_zone_h = 8.min(size.height.saturating_sub(6));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),           // title
            Constraint::Min(9),              // targets + progress row (grows)
            Constraint::Length(1),           // clipboard strip
            Constraint::Length(drop_zone_h), // drop zone (fixed, compact)
            Constraint::Length(1),           // toast
            Constraint::Length(1),           // help bar
        ])
        .split(size);

    let tp_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    app.hit_regions.targets_panel = tp_row[0];
    app.hit_regions.progress_panel = tp_row[1];
    app.hit_regions.drop_zone = chunks[3];

    draw_title(f, chunks[0], app, &palette);
    draw_targets(f, tp_row[0], app, &palette);
    draw_progress(f, tp_row[1], app, &palette);
    draw_clipboard_strip(f, chunks[2], app, &palette);
    draw_drop_zone(f, chunks[3], app, &palette);
    draw_toast(f, chunks[4], app, &palette);
    draw_help_bar(f, chunks[5], app, &palette);

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
    for (i, host) in picker.list.iter().enumerate().take(max_rows) {
        let selected = i == picker.cursor;
        let row_y = rect.y + 1 + (i as u16) + 1;
        let endpoint = format!(
            "{}{}{}",
            host.user.as_deref().unwrap_or(""),
            if host.user.is_some() { "@" } else { "" },
            host.hostname.clone().unwrap_or_else(|| host.name.clone()),
        );
        let label = Span::styled(
            format!("  {}. {}", i + 1, host.name),
            if selected {
                Style::default()
                    .fg(p.bg)
                    .bg(p.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(p.fg)
            },
        );
        let endpoint_span = Span::styled(
            format!("  {endpoint}"),
            Style::default().fg(if selected { p.bg } else { p.muted }),
        );
        lines.push(Line::from(vec![label, endpoint_span]));
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

fn draw_clipboard_strip(f: &mut Frame<'_>, area: Rect, app: &App, p: &theme::Palette) {
    let line = match &app.last_clipboard {
        Some(v) => Line::from(vec![
            Span::styled(
                " ✓ clipboard ",
                Style::default()
                    .fg(p.bg)
                    .bg(p.diff_add)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                truncate_middle(v, area.width.saturating_sub(14) as usize),
                Style::default().fg(p.accent),
            ),
        ]),
        None => Line::from(vec![
            Span::styled(
                " clipboard ",
                Style::default().fg(p.muted).add_modifier(Modifier::DIM),
            ),
            Span::raw(" "),
            Span::styled(
                "(drop a file and the remote path lands here)",
                Style::default().fg(p.muted),
            ),
        ]),
    };
    f.render_widget(
        Paragraph::new(line).style(Style::default().bg(p.bg).fg(p.fg)),
        area,
    );
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
                "  1.  Press Ctrl+P (or click ≡ below)",
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

    // Each transfer takes 2 lines: a caption row with target/file/percent/rate
    // and a bar row with a clearly visible filled/unfilled gauge.
    let recent_budget = (inner.height / 2) as usize;
    let recent: Vec<_> = app
        .transfers
        .iter()
        .rev()
        .take(recent_budget.max(1))
        .collect();
    if recent.is_empty() {
        return;
    }

    let row_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(2); recent.len()])
        .split(inner);

    for (i, t) in recent.iter().enumerate() {
        let slot = row_chunks[i];
        if slot.height < 2 {
            continue;
        }
        let caption_area = Rect::new(slot.x, slot.y, slot.width, 1);
        let bar_area = Rect::new(slot.x, slot.y + 1, slot.width, 1);

        let color = match t.state {
            TransferState::Completed => p.diff_add,
            TransferState::Failed => p.diff_del,
            TransferState::Running => p.accent,
            TransferState::Pending => p.muted,
        };
        let (icon, icon_style) = match t.state {
            TransferState::Completed => (
                "✓",
                Style::default().fg(p.diff_add).add_modifier(Modifier::BOLD),
            ),
            TransferState::Failed => (
                "✗",
                Style::default().fg(p.diff_del).add_modifier(Modifier::BOLD),
            ),
            TransferState::Running => ("●", Style::default().fg(p.accent)),
            TransferState::Pending => ("·", Style::default().fg(p.muted)),
        };

        let filename = t
            .local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Allow up to ~60% of the panel width for the filename so the rate
        // column always fits.
        let fname_budget = (slot.width as usize).saturating_sub(28);
        let filename_short = truncate_middle(&filename, fname_budget.max(10));

        let percent = t.percent.min(100);
        let rate_text = if t.rate.is_empty() || percent == 100 {
            String::new()
        } else {
            format!(" {}", t.rate)
        };
        let pct_text = format!(" {percent:>3}%");

        let caption = Line::from(vec![
            Span::raw(" "),
            Span::styled(icon, icon_style),
            Span::raw(" "),
            Span::styled(
                t.target_name.clone(),
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(filename_short, Style::default().fg(p.fg)),
            Span::raw("  "),
            Span::styled(pct_text, Style::default().fg(color)),
            Span::styled(rate_text, Style::default().fg(p.muted)),
        ]);
        f.render_widget(
            Paragraph::new(caption).style(Style::default().bg(p.bg)),
            caption_area,
        );

        draw_bar(f, bar_area, percent, color, p);
    }
}

/// Render a solid progress bar row with a clearly visible unfilled track.
fn draw_bar(
    f: &mut Frame<'_>,
    area: Rect,
    percent: u8,
    fill: ratatui::style::Color,
    p: &theme::Palette,
) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    let filled = (width as u32 * percent.min(100) as u32 / 100) as usize;
    let unfilled = width.saturating_sub(filled);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(2);
    if filled > 0 {
        spans.push(Span::styled("█".repeat(filled), Style::default().fg(fill)));
    }
    if unfilled > 0 {
        spans.push(Span::styled(
            "─".repeat(unfilled),
            Style::default().fg(p.muted),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(p.bg)),
        area,
    );
}

fn draw_help_bar(f: &mut Frame<'_>, area: Rect, app: &mut App, p: &theme::Palette) {
    // (key-label, description, click action). ≡ opens the menu which contains
    // every less-common action, so we keep the bar short.
    let chips: [(&str, &str, HelpBarAction); 6] = [
        ("Tab", "focus", HelpBarAction::CycleFocus),
        ("Enter", "sync", HelpBarAction::Sync),
        ("u", "update", HelpBarAction::CheckUpdate),
        ("≡", "menu", HelpBarAction::OpenMenu),
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
        Line::from("  Ctrl+P             open menu / command palette"),
        Line::from("  Tab / Shift+Tab    cycle focus"),
        Line::from("  Up / Down          move cursor"),
        Line::from("  Space              toggle selected target"),
        Line::from("  1–9                quick-toggle Nth target"),
        Line::from("  Enter              execute sync (manual)"),
        Line::from("  a / m              auto / manual mode"),
        Line::from("  c                  cycle clipboard format"),
        Line::from("  t                  cycle theme"),
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
    use unicode_width::UnicodeWidthChar;
    s.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}
