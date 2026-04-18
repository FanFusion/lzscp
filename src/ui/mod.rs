pub mod theme;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Focus, TargetKind};
use crate::target::SyncMode;
use crate::transfer::TransferState;

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let palette = theme::palette(&app.cfg.theme);
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),     // title bar
            Constraint::Length(7),     // drop zone
            Constraint::Min(6),        // targets
            Constraint::Length(8),     // progress
            Constraint::Length(2),     // clipboard + toast
            Constraint::Length(1),     // help
        ])
        .split(size);

    draw_title(f, chunks[0], app, &palette);
    draw_drop_zone(f, chunks[1], app, &palette);
    draw_targets(f, chunks[2], app, &palette);
    draw_progress(f, chunks[3], app, &palette);
    draw_status(f, chunks[4], app, &palette);
    draw_help_bar(f, chunks[5], app, &palette);

    if app.help_visible {
        draw_help_overlay(f, size, &palette);
    }
}

fn draw_title(f: &mut Frame<'_>, area: Rect, app: &App, p: &theme::Palette) {
    let mode = match app.mode {
        SyncMode::Auto => "auto",
        SyncMode::Manual => "manual",
    };
    let title = Line::from(vec![
        Span::styled(" lzscp ", Style::default().bg(p.accent).fg(p.bg).add_modifier(Modifier::BOLD)),
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
    f.render_widget(Paragraph::new(title).style(Style::default().fg(p.fg).bg(p.bg)), area);
}

fn draw_drop_zone(f: &mut Frame<'_>, area: Rect, app: &App, p: &theme::Palette) {
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
        f.render_widget(Paragraph::new(hint).block(block).wrap(Wrap { trim: false }), area);
        return;
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

fn draw_targets(f: &mut Frame<'_>, area: Rect, app: &App, p: &theme::Palette) {
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
                Span::styled(format!(" {checkbox} "), Style::default().fg(if row.selected { p.accent } else { p.muted })),
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

fn draw_progress(f: &mut Frame<'_>, area: Rect, app: &App, p: &theme::Palette) {
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

fn draw_status(f: &mut Frame<'_>, area: Rect, app: &App, p: &theme::Palette) {
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

fn draw_help_bar(f: &mut Frame<'_>, area: Rect, _app: &App, p: &theme::Palette) {
    let help = Line::from(vec![
        Span::styled(" Tab ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::styled("focus  ", Style::default().fg(p.muted)),
        Span::styled(" Space ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::styled("toggle  ", Style::default().fg(p.muted)),
        Span::styled(" Enter ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::styled("sync  ", Style::default().fg(p.muted)),
        Span::styled(" a/m ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::styled("auto/manual  ", Style::default().fg(p.muted)),
        Span::styled(" c ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::styled("clipboard-fmt  ", Style::default().fg(p.muted)),
        Span::styled(" ? ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::styled("help  ", Style::default().fg(p.muted)),
        Span::styled(" q ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::styled("quit", Style::default().fg(p.muted)),
    ]);
    f.render_widget(
        Paragraph::new(help).style(Style::default().bg(p.bg)),
        area,
    );
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
