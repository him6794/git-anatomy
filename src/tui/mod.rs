//! TUI: Interactive terminal interface using ratatui (Phase 3)
//!
//! Provides an interactive TUI that allows users to:
//! 1. Browse tracked files and their commit counts
//! 2. Visualize temporal coupling as an interactive map
//! 3. Drill down into function-level coupling and risk levels
//! 4. See commit details for coupled files

mod app;
mod ui;

use anyhow::{Context, Result};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::Path;

use crate::db;
use crate::git_engine;

/// Launch the interactive TUI application.
pub fn run_tui(repo_path: &Path) -> Result<()> {
    // Setup terminal
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)
        .context("Failed to create terminal backend")?;

    // Load data
    let repo = git_engine::open_repo(repo_path)?;
    let commits = git_engine::extract_commit_history(&repo, 0)?;
    let database = db::Database::new()?;
    database.ingest_commits(&commits)?;

    // Create app state
    let mut app = app::App::new(database, repo_path.to_path_buf());
    app.load_data()?;

    // Run the TUI loop
    let res = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    ).context("Failed to leave alternate screen")?;
    terminal.show_cursor().context("Failed to show cursor")?;

    res?;

    Ok(())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut app::App,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        // Handle events
        if event::poll(std::time::Duration::from_millis(100))
            .context("Failed to poll for events")?
        {
            if let Event::Key(key) = event::read().context("Failed to read event")? {
                if key.kind == KeyEventKind::Press {
                    if app.is_searching() {
                        // Search mode keybindings
                        match key.code {
                            KeyCode::Enter => app.finish_search(),
                            KeyCode::Esc => {
                                app.searching = false;
                                app.search_query.clear();
                            }
                            KeyCode::Backspace => app.backspace_search(),
                            KeyCode::Char(c) => app.append_search(c),
                            _ => {}
                        }
                    } else {
                        // Normal mode keybindings
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                            KeyCode::Down | KeyCode::Char('j') => app.next_item(),
                            KeyCode::Up | KeyCode::Char('k') => app.prev_item(),
                            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => app.select_item(),
                            KeyCode::Left | KeyCode::Char('h') => app.go_back(),
                            KeyCode::Char('c') => app.toggle_coupling_view(),
                            KeyCode::Char('f') => app.toggle_function_view(),
                            KeyCode::Char('/') => app.start_search(),
                            KeyCode::Tab => app.next_panel(),
                            KeyCode::BackTab => app.prev_panel(),
                            _ => {}
                        }
                    }
                }
            }
        }

        if app.should_quit() {
            return Ok(());
        }
    }
}
