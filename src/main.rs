mod agent;
mod app;
mod git;
mod keys;
mod names;
mod ports;
mod registry;
mod term;
mod theme;
mod tmux;
mod ui;

use anyhow::{Result, bail};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
};
use std::{
    io::IsTerminal,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use app::App;

const USAGE: &str = "\
maestro — parallel coding agents in isolated git worktrees

usage: maestro [--help] [--version]

  Workspaces are listed in a sidebar; the selected agent fills the rest.
  j/k move   ⏎ insert   n new   g lazygit   s shell   e editor
  p ports    L land     R restart dead      d remove   q quit

environment:
  MAESTRO_REPO_ROOT   where to look for repositories (colon separated)
                      default: ~/Work, ~/code, ~/src, ~/dev, ~/projects, …
  MAESTRO_STATE       registry location (default ~/.local/state/maestro)
  MAESTRO_THEME_FILE  read the palette from a specific colors.toml

  Needs tmux, git, and at least one agent CLI on PATH.
";

fn main() -> Result<()> {
    // A published binary should answer --help rather than panic, and should say
    // what is wrong when there is no terminal to draw on.
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("maestro {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => {
                eprint!("maestro: unknown argument {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }

    if !std::io::stdout().is_terminal() {
        bail!("maestro draws a full-screen interface and needs a terminal");
    }

    registry::ensure_dirs()?;

    let dirty = Arc::new(AtomicBool::new(false));
    let mut app = App::new(dirty);
    app.refresh();

    let mut terminal = ratatui::init();
    // Clicks and scroll in the sidebar; over the agent they are forwarded on.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let result = run(&mut terminal, &mut app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let mut last_refresh = Instant::now();

    loop {
        terminal.draw(|frame| ui::render(frame, app))?;
        // Needs the layout from the frame just drawn, so it runs after the draw.
        app.sync_pty();

        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Mouse(ev) => app.on_mouse(ev),
                _ => {}
            }
        }

        if last_refresh.elapsed() >= Duration::from_millis(1200) {
            app.refresh();
            last_refresh = Instant::now();
        }

        if app.quit {
            break;
        }
    }
    Ok(())
}
