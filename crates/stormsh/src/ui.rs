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
        View::Processes => draw_processes(f, chunks[1], app),
        View::Terminal => draw_terminal(f, chunks[1], app),
        View::Logs => draw_logs(f, chunks[1], app),
    }

    draw_status_bar(f, chunks[2], app);
}

fn draw_tabs(f: &mut Frame, area: Rect, app: &App) {
    let tabs = vec![
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
        InputMode::Normal => " q:Quit  1:Processes  2:Terminal  3:Logs  Enter:Select  s/r:Start/Restart  x:Stop ",
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
