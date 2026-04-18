mod app;
mod clipboard;
mod config;
mod path_input;
mod target;
mod transfer;
mod ui;
mod update;

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    EventStream,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::time::interval;

use crate::app::{App, AppEvent};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("lzscp {VERSION}");
    println!("Lazy SCP — drag-drop file sync to SSH hosts with auto clipboard path return");
    println!();
    println!("USAGE:");
    println!("    lzscp [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help        Print this help");
    println!("    -V, --version     Print version");
    println!("        --check       Check for updates and exit");
    println!();
    println!("CONFIG:");
    println!("    Project:  $PWD/.lzscp/config.toml");
    println!("    Global:   $XDG_CONFIG_HOME/lzscp/config.toml");
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(a) = args.first() {
        match a.as_str() {
            "-V" | "--version" => {
                println!("lzscp {VERSION}");
                return Ok(());
            }
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            "--check" => {
                match update::check_for_updates().await {
                    Ok(Some(v)) => println!("New version available: {v} (current {VERSION})"),
                    Ok(None) => println!("lzscp is up to date ({VERSION})"),
                    Err(e) => eprintln!("Update check failed: {e}"),
                }
                return Ok(());
            }
            other => {
                eprintln!("Unknown argument: {other}");
                print_help();
                std::process::exit(2);
            }
        }
    }

    let cfg = config::load().context("failed to load config")?;
    let mut terminal = setup_terminal().context("failed to init terminal")?;
    let result = run_app(&mut terminal, cfg).await;
    restore_terminal(&mut terminal).ok();
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
    )?;
    terminal.show_cursor()?;
    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    cfg: config::Config,
) -> Result<()> {
    let mut app = App::new(cfg);
    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(100));

    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, &app))?;

        tokio::select! {
            maybe_evt = events.next() => {
                if let Some(Ok(evt)) = maybe_evt {
                    app.handle_event(AppEvent::Terminal(evt));
                }
            }
            _ = tick.tick() => {
                app.tick();
            }
            Some(prog) = app.transfer_rx.recv() => {
                app.handle_event(AppEvent::TransferUpdate(prog));
            }
            Some(app_evt) = app.app_rx.recv() => {
                app.handle_event(app_evt);
            }
        }
    }

    Ok(())
}
