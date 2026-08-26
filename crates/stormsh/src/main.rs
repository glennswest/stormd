mod app;
mod client;
mod ui;

use app::{App, InputMode, View};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "stormsh", version, about = "TUI client for stormd")]
struct Cli {
    /// Host to connect to
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    host: String,

    /// Port to connect to
    #[arg(short, long, default_value = "9080")]
    port: u16,

    /// Bearer token when stormd has auth enabled ([api] auth_token).
    /// Falls back to the STORMD_TOKEN environment variable.
    #[arg(short = 't', long)]
    token: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let token = cli.token.clone().or_else(|| std::env::var("STORMD_TOKEN").ok());
    let client = client::StormClient::new(&cli.host, cli.port, token);

    let mut app = App::new(client);

    // Initial data fetch
    app.refresh_processes().await;
    app.refresh_components().await;

    // Setup terminal
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // Main event loop
    let result = run_app(&mut terminal, &mut app).await;

    // Cleanup
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> anyhow::Result<()> {
    let mut refresh_interval = tokio::time::interval(Duration::from_secs(2));

    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        // Poll for events with timeout
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match app.input_mode {
                    InputMode::Normal => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            app.running = false;
                            return Ok(());
                        }
                        KeyCode::Char('1') => app.view = View::Dashboard,
                        KeyCode::Char('2') => app.view = View::Processes,
                        KeyCode::Char('3') => app.view = View::Terminal,
                        KeyCode::Char('4') => app.view = View::Logs,
                        KeyCode::Tab => {
                            app.view = match app.view {
                                View::Dashboard => View::Processes,
                                View::Processes => View::Terminal,
                                View::Terminal => View::Logs,
                                View::Logs => View::Dashboard,
                            };
                        }
                        KeyCode::Up | KeyCode::Char('k') => match app.view {
                            View::Dashboard => app.dash_prev(),
                            _ => app.select_prev(),
                        },
                        KeyCode::Down | KeyCode::Char('j') => match app.view {
                            View::Dashboard => app.dash_next(),
                            _ => app.select_next(),
                        },
                        KeyCode::Left | KeyCode::Char('h') => {
                            if app.view == View::Dashboard {
                                app.dash_prev();
                            }
                        }
                        KeyCode::Right => {
                            if app.view == View::Dashboard {
                                app.dash_next();
                            }
                        }
                        KeyCode::Enter => {
                            // Open the selected process's terminal — from the
                            // process list, or from a process/plugin card.
                            let name = match app.view {
                                View::Processes => app.selected_process().map(|p| p.name.clone()),
                                View::Dashboard => app
                                    .dash_selected()
                                    .and_then(|c| c.id.strip_prefix("process:"))
                                    .map(|s| s.to_string()),
                                _ => None,
                            };
                            if let Some(name) = name {
                                if let Some(i) =
                                    app.processes.iter().position(|p| p.name == name)
                                {
                                    app.selected_index = i;
                                }
                                match app.client.terminal_snapshot(&name).await {
                                    Ok(snap) => {
                                        app.terminal_content = snap
                                            .get("contents")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        app.view = View::Terminal;
                                    }
                                    Err(e) => {
                                        app.status_message = format!("Error: {}", e);
                                    }
                                }
                            }
                        }
                        KeyCode::Char('s') => match app.view {
                            View::Dashboard => app.dash_action("start").await,
                            _ => {
                                if let Some(p) = app.selected_process() {
                                    let name = p.name.clone();
                                    match app.client.start_process(&name).await {
                                        Ok(()) => {
                                            app.status_message = format!("Started {}", name);
                                        }
                                        Err(e) => {
                                            app.status_message = format!("Error: {}", e);
                                        }
                                    }
                                    app.refresh_processes().await;
                                }
                            }
                        },
                        KeyCode::Char('x') => match app.view {
                            View::Dashboard => app.dash_action("stop").await,
                            _ => {
                                if let Some(p) = app.selected_process() {
                                    let name = p.name.clone();
                                    match app.client.stop_process(&name).await {
                                        Ok(()) => {
                                            app.status_message = format!("Stopped {}", name);
                                        }
                                        Err(e) => {
                                            app.status_message = format!("Error: {}", e);
                                        }
                                    }
                                    app.refresh_processes().await;
                                }
                            }
                        },
                        KeyCode::Char('r') => match app.view {
                            View::Dashboard => app.dash_action("restart").await,
                            _ => {
                                if let Some(p) = app.selected_process() {
                                    let name = p.name.clone();
                                    match app.client.restart_process(&name).await {
                                        Ok(()) => {
                                            app.status_message = format!("Restarted {}", name);
                                        }
                                        Err(e) => {
                                            app.status_message = format!("Error: {}", e);
                                        }
                                    }
                                    app.refresh_processes().await;
                                }
                            }
                        },
                        KeyCode::Char('u') => {
                            if app.view == View::Dashboard {
                                app.dash_action("trigger").await;
                            }
                        }
                        KeyCode::Char('l') => {
                            app.view = View::Logs;
                        }
                        KeyCode::Char('t') => {
                            app.view = View::Terminal;
                        }
                        _ => {}
                    },
                    InputMode::Search => match key.code {
                        KeyCode::Esc => {
                            app.input_mode = InputMode::Normal;
                            app.search_query.clear();
                        }
                        KeyCode::Enter => {
                            app.input_mode = InputMode::Normal;
                            // Apply search filter to logs
                        }
                        KeyCode::Backspace => {
                            app.search_query.pop();
                        }
                        KeyCode::Char(c) => {
                            app.search_query.push(c);
                        }
                        _ => {}
                    },
                }
            }
        }

        // Periodic refresh
        if refresh_interval.tick().await.elapsed() > Duration::ZERO {
            app.refresh_processes().await;
            app.refresh_components().await;
            app.status_message.clear();
        }
    }
}
