use crate::app::{App, InputMode, View};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab bar
            Constraint::Min(0),   // main content
            Constraint::Length(1), // status bar
        ])
        .split(f.area());

    draw_tabs(f, chunks[0], app);

    match app.view {
        View::Dashboard => draw_dashboard(f, chunks[1], app),
        View::Processes => draw_processes(f, chunks[1], app),
        View::Terminal => draw_terminal(f, chunks[1], app),
        View::Logs => draw_logs(f, chunks[1], app),
    }

    draw_status_bar(f, chunks[2], app);
}

fn draw_tabs(f: &mut Frame, area: Rect, app: &App) {
    let tabs = vec![
        ("Dashboard", View::Dashboard),
        ("Processes", View::Processes),
        ("Terminal", View::Terminal),
        ("Logs", View::Logs),
    ];

    let spans: Vec<Span> = tabs
        .iter()
        .flat_map(|(name, view)| {
            let style = if *view == app.view {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            vec![
                Span::styled(format!(" {} ", name), style),
                Span::raw(" "),
            ]
        })
        .collect();

    let tabs_line = Line::from(spans);
    f.render_widget(Paragraph::new(tabs_line), area);
}

/// The console "sum" of every component: the same summaries the web
/// dashboard renders, drawn as a grid of tiles. The tile knows nothing about
/// kinds — id, health, detail, and metrics come from the feed.
fn draw_dashboard(f: &mut Frame, area: Rect, app: &App) {
    if app.components.is_empty() {
        let msg = if app.connected {
            "No components reported."
        } else {
            "Not connected."
        };
        f.render_widget(
            Paragraph::new(msg).block(Block::default().borders(Borders::ALL).title(" Dashboard ")),
            area,
        );
        return;
    }

    const TILE_H: u16 = 6;
    const TILE_MIN_W: u16 = 34;
    let cols = ((area.width / TILE_MIN_W).max(1)) as usize;
    let rows_fit = ((area.height / TILE_H).max(1)) as usize;
    let total_rows = app.components.len().div_ceil(cols);

    // Keep the selected tile's row visible.
    let sel_row = app.dash_index / cols;
    let first_row = if sel_row >= rows_fit {
        sel_row + 1 - rows_fit
    } else {
        0
    };

    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(TILE_H); rows_fit])
        .split(area);

    for (r, row_area) in row_areas.iter().enumerate() {
        let row = first_row + r;
        if row >= total_rows {
            break;
        }
        let col_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Ratio(1, cols as u32); cols])
            .split(*row_area);
        for (c, col_area) in col_areas.iter().enumerate() {
            let i = row * cols + c;
            let Some(component) = app.components.get(i) else { break };
            draw_tile(f, *col_area, component, i == app.dash_index);
        }
    }
}

fn health_color(health: stormview::Health) -> Color {
    use stormview::Health;
    match health {
        Health::Ok => Color::Green,
        Health::Warn => Color::Yellow,
        Health::Error => Color::Red,
        Health::Idle | Health::Unknown => Color::DarkGray,
    }
}

fn draw_tile(f: &mut Frame, area: Rect, c: &stormview::ComponentSummary, selected: bool) {
    let border_style = if selected {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = Line::from(vec![
        Span::styled(" ● ", Style::default().fg(health_color(c.health))),
        Span::styled(
            c.label.clone(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" [{}] ", c.kind), Style::default().fg(Color::DarkGray)),
    ]);

    let metrics: Vec<Span> = c
        .metrics
        .iter()
        .enumerate()
        .flat_map(|(i, m)| {
            let value_style = match m.tone.as_deref() {
                Some("ok") => Style::default().fg(Color::Green),
                Some("warn") => Style::default().fg(Color::Yellow),
                Some("error") => Style::default().fg(Color::Red),
                Some("muted") => Style::default().fg(Color::DarkGray),
                Some("accent") => Style::default().fg(Color::Cyan),
                _ => Style::default().fg(Color::White),
            };
            let mut spans = Vec::new();
            if i > 0 {
                spans.push(Span::styled("  ", Style::default()));
            }
            spans.push(Span::styled(
                format!("{}: ", m.label),
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::styled(
                format!("{}{}", m.value, m.unit.as_deref().unwrap_or("")),
                value_style,
            ));
            spans
        })
        .collect();

    let actions: Vec<Span> = c
        .actions
        .iter()
        .filter(|a| a.enabled)
        .enumerate()
        .flat_map(|(i, a)| {
            let mut spans = Vec::new();
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                format!("[{}]", a.label),
                Style::default().fg(if a.danger { Color::Red } else { Color::Green }),
            ));
            spans
        })
        .collect();

    let mut lines = vec![
        Line::from(Span::styled(
            c.detail.clone(),
            Style::default().fg(Color::Gray),
        )),
        Line::from(metrics),
    ];
    if selected && !actions.is_empty() {
        lines.push(Line::from(actions));
    }

    let tile = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(tile, area);
}

fn draw_processes(f: &mut Frame, area: Rect, app: &App) {
    let header = Row::new(vec![
        Cell::from("PROCESS").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("STATE").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("PID").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("EXIT").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("RESTARTS").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("UPTIME").style(Style::default().add_modifier(Modifier::BOLD)),
    ])
    .height(1);

    let rows: Vec<Row> = app
        .processes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let state_style = match p.state.as_str() {
                "running" => Style::default().fg(Color::Green),
                "failed" => Style::default().fg(Color::Red),
                "stopped" => Style::default().fg(Color::Yellow),
                "restarting" => Style::default().fg(Color::Cyan),
                _ => Style::default().fg(Color::DarkGray),
            };

            let pid = p.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
            let exit = p
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".into());
            let uptime = p
                .uptime_secs
                .map(format_duration)
                .unwrap_or_else(|| "-".into());

            let row_style = if i == app.selected_index {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(p.name.clone()),
                Cell::from(p.state.clone()).style(state_style),
                Cell::from(pid),
                Cell::from(exit),
                Cell::from(p.restarts.to_string()),
                Cell::from(uptime),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(25),
            Constraint::Percentage(15),
            Constraint::Percentage(12),
            Constraint::Percentage(10),
            Constraint::Percentage(13),
            Constraint::Percentage(25),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Processes "),
    );

    f.render_widget(table, area);
}

fn draw_terminal(f: &mut Frame, area: Rect, app: &App) {
    let title = match app.selected_process() {
        Some(p) => format!(" Terminal — {} ", p.name),
        None => " Terminal — (select a process) ".to_string(),
    };

    let text = if app.terminal_content.is_empty() {
        "No terminal content. Select a process and press Enter.".to_string()
    } else {
        app.terminal_content.clone()
    };

    let paragraph = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn draw_logs(f: &mut Frame, area: Rect, app: &App) {
    let log_lines = app.log_lines.blocking_lock();
    let lines: Vec<Line> = log_lines
        .iter()
        .map(|l| {
            let style = if l.contains("[error]") || l.contains("[stderr]") {
                Style::default().fg(Color::Red)
            } else if l.contains("[warning]") {
                Style::default().fg(Color::Yellow)
            } else if l.contains("[debug]") {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };
            Line::styled(l.clone(), style)
        })
        .collect();

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Logs "))
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let conn_status = if app.connected {
        Span::styled("CONNECTED", Style::default().fg(Color::Green))
    } else {
        Span::styled("DISCONNECTED", Style::default().fg(Color::Red))
    };

    let help = match app.input_mode {
        InputMode::Normal => match app.view {
            View::Dashboard => " q:Quit  1-4:Views  ←↑↓→:Select  Enter:Terminal  s/r/x:Start/Restart/Stop  u:Update ",
            _ => " q:Quit  1:Dashboard  2:Processes  3:Terminal  4:Logs  Enter:Select  s/r:Start/Restart  x:Stop ",
        },
        InputMode::Search => " ESC:Cancel  Enter:Search ",
    };

    let status = Line::from(vec![
        Span::raw(" "),
        conn_status,
        Span::raw(" | "),
        Span::styled(
            if !app.status_message.is_empty() {
                app.status_message.clone()
            } else {
                help.to_string()
            },
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    f.render_widget(
        Paragraph::new(status).style(Style::default().bg(Color::Black)),
        area,
    );
}

fn format_duration(secs: i64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, mins, s)
    } else if mins > 0 {
        format!("{}m {}s", mins, s)
    } else {
        format!("{}s", s)
    }
}
