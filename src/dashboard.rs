// Ratatui app loop, event handling, debounce
#![allow(dead_code)]

use crate::registry::{self, Session, Status};
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::Terminal;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Kanban columns displayed in the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    Waiting,
    Working,
    Idle,
    Done,
}

/// Dashboard input mode — Normal for kanban navigation, NewSession for the modal.
#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    NewSession,
}

/// Column display order (left to right).
pub const COLUMN_ORDER: [Column; 4] =
    [Column::Waiting, Column::Working, Column::Idle, Column::Done];

/// Debounce duration for Notification → waiting transitions per PRD §8.
/// If a PreToolUse event arrives within this window, the session stays
/// visually in `working` and never flashes yellow.
const DEBOUNCE_DURATION: Duration = Duration::from_secs(5);

impl Column {
    /// Column header title.
    pub fn title(self) -> &'static str {
        match self {
            Column::Waiting => "NEEDS INPUT",
            Column::Working => "WORKING",
            Column::Idle => "IDLE",
            Column::Done => "DONE",
        }
    }

    /// Map a session status to its kanban column.
    pub fn from_status(status: &Status) -> Column {
        match status {
            Status::Waiting => Column::Waiting,
            Status::Starting | Status::Working => Column::Working,
            Status::Idle => Column::Idle,
            Status::Done => Column::Done,
        }
    }
}

/// Dashboard application state.
pub struct App {
    /// All sessions keyed by name.
    pub sessions: HashMap<String, Session>,
    /// Index of selected column within visible_columns().
    pub selected_column: usize,
    /// Selected row index per column.
    pub selected_rows: HashMap<Column, usize>,
    /// File watcher event receiver.
    watcher_rx: mpsc::Receiver<notify::Result<notify::Event>>,
    /// File watcher handle (must stay alive).
    _watcher: RecommendedWatcher,
    /// Registry directory being watched.
    registry_dir: PathBuf,
    /// Whether the app should quit.
    pub should_quit: bool,
    /// Debounce timers: session name -> time the waiting event arrived.
    pub debounce_timers: HashMap<String, Instant>,
    /// Current input mode (normal navigation vs. modal input).
    pub input_mode: InputMode,
    /// Text buffer for the session name field in the new-session modal.
    pub modal_name: String,
    /// Text buffer for the directory field in the new-session modal.
    pub modal_dir: String,
    /// Which field is active in the modal: 0 = name, 1 = directory.
    pub modal_field: usize,
    /// Validation error message to display in the modal, if any.
    pub modal_error: Option<String>,
    /// Default working directory for new sessions (cwd of the dashboard process).
    pub default_cwd: String,
    /// Session name to auto-focus on next watcher reload (set when creating from modal).
    pub pending_focus: Option<String>,
}

impl App {
    /// Create a new App watching the default registry directory.
    pub fn new() -> Result<Self> {
        let dir = registry::registry_dir()?;
        Self::with_registry_dir(&dir)
    }

    /// Create a new App watching a specific registry directory.
    pub fn with_registry_dir(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create registry dir: {}", dir.display()))?;

        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .context("failed to create file watcher")?;

        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .with_context(|| format!("failed to watch {}", dir.display()))?;

        let sessions = load_sessions_from(dir);

        let mut app = App {
            sessions,
            selected_column: 0,
            selected_rows: HashMap::new(),
            watcher_rx: rx,
            _watcher: watcher,
            registry_dir: dir.to_path_buf(),
            should_quit: false,
            debounce_timers: HashMap::new(),
            input_mode: InputMode::Normal,
            modal_name: String::new(),
            modal_dir: String::new(),
            modal_field: 0,
            modal_error: None,
            default_cwd: std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            pending_focus: None,
        };

        app.focus_initial_column();
        Ok(app)
    }

    /// Returns the effective display column for a session, considering debounce.
    /// During the debounce window, a session that is `Waiting` in the file
    /// is shown in the `Working` column to prevent false "needs input" flashes.
    fn effective_column(&self, name: &str, session: &Session) -> Column {
        if session.status == Status::Waiting
            && let Some(timer_start) = self.debounce_timers.get(name)
            && timer_start.elapsed() < DEBOUNCE_DURATION
        {
            return Column::Working;
        }
        Column::from_status(&session.status)
    }

    /// Returns columns that have at least one session, in display order.
    pub fn visible_columns(&self) -> Vec<Column> {
        COLUMN_ORDER
            .iter()
            .copied()
            .filter(|col| {
                self.sessions
                    .iter()
                    .any(|(name, s)| self.effective_column(name, s) == *col)
            })
            .collect()
    }

    /// Returns (name, session) pairs for a column, sorted by age ascending (oldest first).
    pub fn sessions_in_column(&self, col: Column) -> Vec<(&str, &Session)> {
        let mut entries: Vec<_> = self
            .sessions
            .iter()
            .filter(|(name, s)| self.effective_column(name, s) == col)
            .map(|(name, session)| (name.as_str(), session))
            .collect();
        entries.sort_by_key(|(_, s)| s.ts);
        entries
    }

    /// Drain file watcher events and reload sessions if anything changed.
    /// Manages debounce timers: starts a timer when a session transitions
    /// to `waiting`, cancels it if the session leaves `waiting` within the window.
    pub fn process_watcher_events(&mut self) {
        let mut changed = false;
        while self.watcher_rx.try_recv().is_ok() {
            changed = true;
        }
        if changed {
            // Snapshot which sessions were already waiting before reload
            let previously_waiting: Vec<String> = self
                .sessions
                .iter()
                .filter(|(_, s)| s.status == Status::Waiting)
                .map(|(name, _)| name.clone())
                .collect();

            self.sessions = load_sessions_from(&self.registry_dir);

            // Manage debounce timers for waiting transitions
            for (name, session) in &self.sessions {
                if session.status == Status::Waiting {
                    // Start debounce only for newly-waiting sessions
                    if !previously_waiting.contains(name)
                        && !self.debounce_timers.contains_key(name)
                    {
                        self.debounce_timers.insert(name.clone(), Instant::now());
                    }
                } else {
                    // No longer waiting → cancel debounce
                    self.debounce_timers.remove(name);
                }
            }

            // Clean up timers for removed sessions
            self.debounce_timers
                .retain(|name, _| self.sessions.contains_key(name));

            self.clamp_selections();

            // Auto-focus a session created from the dashboard modal
            if let Some(ref focus_name) = self.pending_focus
                && self.sessions.contains_key(focus_name)
            {
                let focus_name = focus_name.clone();
                self.pending_focus = None;
                // Find which column it's in and focus it
                let col = self
                    .sessions
                    .get(&focus_name)
                    .map(|s| self.effective_column(&focus_name, s))
                    .unwrap_or(Column::Working);
                let visible = self.visible_columns();
                if let Some(col_idx) = visible.iter().position(|c| *c == col) {
                    self.selected_column = col_idx;
                    let entries = self.sessions_in_column(col);
                    if let Some(row) = entries.iter().position(|(n, _)| *n == focus_name) {
                        self.selected_rows.insert(col, row);
                    }
                }
            }
        }
    }

    /// Ensure selected_column and selected_rows are within bounds.
    fn clamp_selections(&mut self) {
        let visible = self.visible_columns();
        if visible.is_empty() {
            self.selected_column = 0;
        } else if self.selected_column >= visible.len() {
            self.selected_column = visible.len() - 1;
        }
        for col in &COLUMN_ORDER {
            let count = self.sessions_in_column(*col).len();
            if let Some(row) = self.selected_rows.get_mut(col) {
                if count == 0 {
                    *row = 0;
                } else if *row >= count {
                    *row = count - 1;
                }
            }
        }
    }

    /// Set initial column focus: prefer Waiting, then first visible.
    fn focus_initial_column(&mut self) {
        let visible = self.visible_columns();
        if let Some(idx) = visible.iter().position(|c| *c == Column::Waiting) {
            self.selected_column = idx;
        } else {
            self.selected_column = 0;
        }
    }

    /// Get the currently selected column, if any visible columns exist.
    pub fn current_column(&self) -> Option<Column> {
        let visible = self.visible_columns();
        visible.get(self.selected_column).copied()
    }

    /// Get the currently selected session name, if any.
    pub fn selected_session(&self) -> Option<&str> {
        let col = self.current_column()?;
        let entries = self.sessions_in_column(col);
        let row = self.selected_rows.get(&col).copied().unwrap_or(0);
        entries.get(row).map(|(name, _)| *name)
    }

    /// Move selection down within the current column (j key).
    pub fn move_down(&mut self) {
        let Some(col) = self.current_column() else {
            return;
        };
        let count = self.sessions_in_column(col).len();
        if count == 0 {
            return;
        }
        let row = self.selected_rows.entry(col).or_insert(0);
        if *row + 1 < count {
            *row += 1;
        }
    }

    /// Move selection up within the current column (k key).
    pub fn move_up(&mut self) {
        let Some(col) = self.current_column() else {
            return;
        };
        let row = self.selected_rows.entry(col).or_insert(0);
        if *row > 0 {
            *row -= 1;
        }
    }

    /// Move selection to the previous visible column (h key).
    pub fn move_left(&mut self) {
        if self.visible_columns().is_empty() {
            return;
        }
        if self.selected_column > 0 {
            self.selected_column -= 1;
        }
    }

    /// Move selection to the next visible column (l key).
    pub fn move_right(&mut self) {
        let visible = self.visible_columns();
        if visible.is_empty() {
            return;
        }
        if self.selected_column + 1 < visible.len() {
            self.selected_column += 1;
        }
    }

    /// Process debounce timers: remove expired timers and return session names
    /// that have completed the debounce period and are now truly `waiting`.
    /// The caller should trigger auto-focus for these sessions.
    pub fn process_debounce_timers(&mut self) -> Vec<String> {
        let mut newly_waiting = Vec::new();
        self.debounce_timers.retain(|name, timer_start| {
            if timer_start.elapsed() >= DEBOUNCE_DURATION {
                // Timer expired — session has been waiting long enough
                if let Some(session) = self.sessions.get(name)
                    && session.status == Status::Waiting
                {
                    newly_waiting.push(name.clone());
                }
                false // remove expired timer
            } else {
                true // keep active timer
            }
        });
        // Column assignments may have changed, re-clamp
        if !newly_waiting.is_empty() {
            self.clamp_selections();
        }
        newly_waiting
    }

    /// Auto-focus: jump selection to the Waiting column and the given session.
    /// Called when a session completes its debounce and is now truly `waiting`.
    pub fn auto_focus_session(&mut self, name: &str) {
        let visible = self.visible_columns();
        if let Some(idx) = visible.iter().position(|c| *c == Column::Waiting) {
            self.selected_column = idx;
            let entries = self.sessions_in_column(Column::Waiting);
            if let Some(row) = entries.iter().position(|(n, _)| *n == name) {
                self.selected_rows.insert(Column::Waiting, row);
            }
        }
    }

    /// Open the new-session modal, pre-filling the directory with the given default.
    pub fn open_new_session_modal(&mut self, default_dir: &str) {
        self.input_mode = InputMode::NewSession;
        self.modal_name.clear();
        self.modal_dir = default_dir.to_string();
        self.modal_field = 0;
        self.modal_error = None;
    }

    /// Close the new-session modal and return to normal mode.
    pub fn close_modal(&mut self) {
        self.input_mode = InputMode::Normal;
        self.modal_name.clear();
        self.modal_dir.clear();
        self.modal_field = 0;
        self.modal_error = None;
    }

    /// Returns the currently active modal text buffer (name or dir).
    pub fn active_modal_buffer(&self) -> &str {
        if self.modal_field == 0 {
            &self.modal_name
        } else {
            &self.modal_dir
        }
    }

    /// Push a character to the active modal field.
    pub fn modal_push_char(&mut self, c: char) {
        self.modal_error = None;
        if self.modal_field == 0 {
            self.modal_name.push(c);
        } else {
            self.modal_dir.push(c);
        }
    }

    /// Delete the last character from the active modal field.
    pub fn modal_pop_char(&mut self) {
        self.modal_error = None;
        if self.modal_field == 0 {
            self.modal_name.pop();
        } else {
            self.modal_dir.pop();
        }
    }

    /// Toggle between name (0) and directory (1) fields.
    pub fn modal_toggle_field(&mut self) {
        self.modal_field = if self.modal_field == 0 { 1 } else { 0 };
    }

    /// Expand `~` to `$HOME` in a path string.
    fn expand_tilde(path: &str) -> String {
        if (path == "~" || path.starts_with("~/"))
            && let Ok(home) = std::env::var("HOME")
        {
            return path.replacen('~', &home, 1);
        }
        path.to_string()
    }

    /// Validate modal inputs. Returns Ok((name, dir)) or sets modal_error and returns Err.
    pub fn validate_modal(&mut self) -> std::result::Result<(String, String), ()> {
        let name = self.modal_name.trim().to_string();
        let dir = Self::expand_tilde(self.modal_dir.trim());

        // Validate name
        if let Err(e) = registry::validate_session_name(&name) {
            self.modal_error = Some(e.to_string());
            return Err(());
        }

        // Check for duplicate
        if self.sessions.contains_key(&name) {
            self.modal_error = Some(format!("session '{name}' already exists"));
            return Err(());
        }

        // Validate directory exists
        if !std::path::Path::new(&dir).is_dir() {
            self.modal_error = Some("directory does not exist".to_string());
            return Err(());
        }

        Ok((name, dir))
    }
}

/// Run the dashboard TUI event loop per PRD §10.
///
/// Single-threaded poll loop: crossterm::event::poll() with 1-second timeout.
/// Each tick: drain keyboard events, drain file watcher events, process debounce.
pub fn run() -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)
        .context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new()?;
    let result = run_loop(&mut terminal, &mut app);

    // Restore terminal (always runs, even on error)
    let _ = disable_raw_mode();
    let _ = crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen);

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| {
            let area = frame.area();
            let modal_height = if app.input_mode == InputMode::NewSession { 4 } else { 2 };
            let chunks =
                Layout::vertical([Constraint::Min(0), Constraint::Length(modal_height)])
                    .split(area);

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            crate::ui::kanban::render_kanban(app, chunks[0], frame.buffer_mut(), now);
            if app.input_mode == InputMode::NewSession {
                crate::ui::modal::render_modal(app, chunks[1], frame.buffer_mut());
            } else {
                crate::ui::statusbar::render_statusbar(app, chunks[1], frame.buffer_mut());
            }
        })?;

        // Poll for keyboard events (1-second timeout for age/debounce refresh)
        if event::poll(Duration::from_secs(1))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match app.input_mode {
                InputMode::Normal => match key.code {
                    KeyCode::Char('q') => app.should_quit = true,
                    KeyCode::Char('j') => app.move_down(),
                    KeyCode::Char('k') => app.move_up(),
                    KeyCode::Char('h') => app.move_left(),
                    KeyCode::Char('l') => app.move_right(),
                    KeyCode::Char('n') => {
                        let cwd = app.default_cwd.clone();
                        app.open_new_session_modal(&cwd);
                    }
                    KeyCode::Enter => {
                        if let Some(name) = app.selected_session() {
                            let name = name.to_owned();
                            let _ = crate::zellij::go_to_tab(&name);
                        }
                    }
                    KeyCode::Char('x') => {
                        if let Some(name) = app.selected_session() {
                            let name = name.to_owned();
                            let _ = crate::zellij::close_tab(&name);
                            let _ = registry::remove_session(&name);
                        }
                    }
                    _ => {}
                },
                InputMode::NewSession => match key.code {
                    KeyCode::Esc => app.close_modal(),
                    KeyCode::Tab | KeyCode::BackTab => app.modal_toggle_field(),
                    KeyCode::Backspace => app.modal_pop_char(),
                    KeyCode::Enter => {
                        if let Ok((name, dir)) = app.validate_modal() {
                            let env_var = format!("CCMUX_SESSION={name}");
                            let result = crate::zellij::new_tab(
                                &name,
                                "env",
                                &[&env_var, "claude", "--dangerously-skip-permissions"],
                                Some(&dir),
                            );
                            if let Err(e) = result {
                                app.modal_error = Some(format!("failed to create session: {e}"));
                            } else {
                                app.pending_focus = Some(name.clone());
                                app.close_modal();
                            }
                        }
                    }
                    KeyCode::Char(c) => app.modal_push_char(c),
                    _ => {}
                },
            }
        }

        // Drain file watcher events and reload sessions
        app.process_watcher_events();

        // Process debounce timers and auto-focus newly waiting sessions
        let newly_waiting = app.process_debounce_timers();
        if let Some(name) = newly_waiting.last() {
            app.auto_focus_session(name);
        }
    }

    Ok(())
}

/// Return the status icon for a session per PRD §8.
/// `?` waiting, `●` working/starting, `○` idle, `✓` done.
pub fn status_icon(status: &Status) -> &'static str {
    match status {
        Status::Waiting => "?",
        Status::Starting | Status::Working => "●",
        Status::Idle => "○",
        Status::Done => "✓",
    }
}

/// Format a session's age relative to `now` as a compact string.
/// Returns seconds (e.g. "43s"), minutes ("14m"), or hours ("2h").
pub fn format_age(ts: u64, now: u64) -> String {
    let elapsed = now.saturating_sub(ts);
    if elapsed < 60 {
        format!("{}s", elapsed)
    } else if elapsed < 3600 {
        format!("{}m", elapsed / 60)
    } else {
        format!("{}h", elapsed / 3600)
    }
}

/// Return the style for a status icon per PRD §8 colour scheme.
pub fn status_style(status: &Status) -> Style {
    match status {
        Status::Waiting => Style::default().fg(Color::Yellow),
        Status::Starting | Status::Working => Style::default().fg(Color::Blue),
        Status::Idle => Style::default().fg(Color::DarkGray),
        Status::Done => Style::default().fg(Color::Green),
    }
}

/// Style for the selected row: dark blue background.
pub fn selected_style() -> Style {
    Style::default().bg(Color::Blue)
}

/// Style for tool name display.
pub fn tool_style() -> Style {
    Style::default().fg(Color::Cyan)
}

/// Style for directory display.
pub fn dir_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Style for a session's message text, varying by status.
pub fn msg_style(status: &Status) -> Style {
    match status {
        Status::Waiting => Style::default().fg(Color::Yellow),
        _ => Style::default().fg(Color::Gray),
    }
}

/// Style for the age display.
pub fn age_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Load all sessions from a directory into a HashMap.
fn load_sessions_from(dir: &Path) -> HashMap<String, Session> {
    registry::list_sessions_from(dir)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::write_session_to;

    fn make_session(status: Status, ts: u64) -> Session {
        Session {
            status,
            tool: None,
            msg: None,
            ts,
            seq: 0,
            dir: None,
        }
    }

    #[test]
    fn column_from_status_mapping() {
        assert_eq!(Column::from_status(&Status::Waiting), Column::Waiting);
        assert_eq!(Column::from_status(&Status::Working), Column::Working);
        assert_eq!(Column::from_status(&Status::Starting), Column::Working);
        assert_eq!(Column::from_status(&Status::Idle), Column::Idle);
        assert_eq!(Column::from_status(&Status::Done), Column::Done);
    }

    #[test]
    fn column_titles() {
        assert_eq!(Column::Waiting.title(), "NEEDS INPUT");
        assert_eq!(Column::Working.title(), "WORKING");
        assert_eq!(Column::Idle.title(), "IDLE");
        assert_eq!(Column::Done.title(), "DONE");
    }

    #[test]
    fn empty_app_has_no_visible_columns() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert!(app.visible_columns().is_empty());
        assert!(app.current_column().is_none());
        assert!(app.selected_session().is_none());
    }

    #[test]
    fn sessions_grouped_into_correct_columns() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Waiting, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();
        write_session_to(dir.path(), "c", &make_session(Status::Idle, 300)).unwrap();
        write_session_to(dir.path(), "d", &make_session(Status::Done, 400)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();

        assert_eq!(app.sessions_in_column(Column::Waiting).len(), 1);
        assert_eq!(app.sessions_in_column(Column::Working).len(), 1);
        assert_eq!(app.sessions_in_column(Column::Idle).len(), 1);
        assert_eq!(app.sessions_in_column(Column::Done).len(), 1);
    }

    #[test]
    fn visible_columns_hides_empty() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Waiting, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Done, 200)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.visible_columns(), vec![Column::Waiting, Column::Done]);
    }

    #[test]
    fn initial_focus_prefers_waiting_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Waiting, 200)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.current_column(), Some(Column::Waiting));
    }

    #[test]
    fn initial_focus_falls_back_to_first_visible() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.current_column(), Some(Column::Working));
    }

    #[test]
    fn sessions_sorted_by_ts_ascending() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "newer", &make_session(Status::Working, 300)).unwrap();
        write_session_to(dir.path(), "older", &make_session(Status::Working, 100)).unwrap();
        write_session_to(dir.path(), "middle", &make_session(Status::Working, 200)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let names: Vec<_> = app
            .sessions_in_column(Column::Working)
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(names, vec!["older", "middle", "newer"]);
    }

    #[test]
    fn selected_session_returns_first_in_focused_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 100)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.selected_session(), Some("sess"));
    }

    #[test]
    fn starting_status_groups_with_working() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "init", &make_session(Status::Starting, 100)).unwrap();
        write_session_to(dir.path(), "run", &make_session(Status::Working, 200)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions_in_column(Column::Working).len(), 2);
    }

    #[test]
    fn watcher_detects_new_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        assert!(app.sessions.is_empty());

        write_session_to(dir.path(), "new", &make_session(Status::Working, 100)).unwrap();

        // Give the watcher a moment to deliver the event
        std::thread::sleep(std::time::Duration::from_millis(200));

        app.process_watcher_events();
        assert_eq!(app.sessions.len(), 1);
        assert!(app.sessions.contains_key("new"));
    }

    #[test]
    fn watcher_detects_removed_session_file() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "gone", &make_session(Status::Idle, 100)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions.len(), 1);

        std::fs::remove_file(dir.path().join("gone.json")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        app.process_watcher_events();
        assert!(app.sessions.is_empty());
    }

    #[test]
    fn move_down_increments_row() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.selected_rows.get(&Column::Working).copied().unwrap_or(0), 0);
        app.move_down();
        assert_eq!(app.selected_rows.get(&Column::Working).copied(), Some(1));
    }

    #[test]
    fn move_down_clamps_at_bottom() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.move_down(); // already at last (only) item
        assert_eq!(app.selected_rows.get(&Column::Working).copied().unwrap_or(0), 0);
    }

    #[test]
    fn move_up_decrements_row() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.selected_rows.insert(Column::Working, 1);
        app.move_up();
        assert_eq!(app.selected_rows.get(&Column::Working).copied(), Some(0));
    }

    #[test]
    fn move_up_clamps_at_top() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.move_up(); // already at 0
        assert_eq!(app.selected_rows.get(&Column::Working).copied().unwrap_or(0), 0);
    }

    #[test]
    fn move_right_advances_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Waiting, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        // Initial focus is Waiting (index 0)
        assert_eq!(app.current_column(), Some(Column::Waiting));
        app.move_right();
        assert_eq!(app.current_column(), Some(Column::Working));
    }

    #[test]
    fn move_right_clamps_at_last_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Waiting, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.move_right();
        app.move_right(); // already at last column
        assert_eq!(app.current_column(), Some(Column::Working));
    }

    #[test]
    fn move_left_retreats_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Waiting, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.selected_column = 1; // Working
        assert_eq!(app.current_column(), Some(Column::Working));
        app.move_left();
        assert_eq!(app.current_column(), Some(Column::Waiting));
    }

    #[test]
    fn move_left_clamps_at_first_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Waiting, 100)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.move_left(); // already at 0
        assert_eq!(app.current_column(), Some(Column::Waiting));
    }

    #[test]
    fn navigation_skips_empty_columns() {
        let dir = tempfile::tempdir().unwrap();
        // Only Waiting and Done exist — Working and Idle are empty/hidden
        write_session_to(dir.path(), "a", &make_session(Status::Waiting, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Done, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.current_column(), Some(Column::Waiting));
        app.move_right();
        // Should jump straight to Done, skipping empty Working and Idle
        assert_eq!(app.current_column(), Some(Column::Done));
    }

    #[test]
    fn navigation_noop_on_empty_app() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        // All should be no-ops
        app.move_up();
        app.move_down();
        app.move_left();
        app.move_right();
        assert!(app.current_column().is_none());
    }

    #[test]
    fn clamp_selections_after_removal() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        // Select second row in Working column
        app.selected_rows.insert(Column::Working, 1);

        // Remove both sessions
        std::fs::remove_file(dir.path().join("a.json")).unwrap();
        std::fs::remove_file(dir.path().join("b.json")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        app.process_watcher_events();
        // selected_rows for Working should be clamped to 0
        assert_eq!(app.selected_rows.get(&Column::Working).copied(), Some(0));
    }

    // --- Debounce tests ---

    #[test]
    fn initial_waiting_sessions_not_debounced() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 100)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        // No debounce timer for sessions loaded at startup
        assert!(!app.debounce_timers.contains_key("sess"));
        // Should appear directly in Waiting column
        assert_eq!(app.sessions_in_column(Column::Waiting).len(), 1);
        assert_eq!(app.sessions_in_column(Column::Working).len(), 0);
    }

    #[test]
    fn debounce_new_waiting_starts_timer() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "sess", &make_session(Status::Working, 100)).unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();

        // Transition to waiting
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 101)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.process_watcher_events();

        // Timer should be set
        assert!(app.debounce_timers.contains_key("sess"));
    }

    #[test]
    fn debounce_keeps_session_in_working_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "sess", &make_session(Status::Working, 100)).unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();

        // Transition to waiting
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 101)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.process_watcher_events();

        // During debounce, session should stay in Working column
        assert_eq!(app.sessions_in_column(Column::Working).len(), 1);
        assert_eq!(app.sessions_in_column(Column::Waiting).len(), 0);
    }

    #[test]
    fn debounce_expired_timer_shows_waiting() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 100)).unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();

        // Manually insert an expired debounce timer (6s ago > 5s threshold)
        app.debounce_timers
            .insert("sess".to_string(), Instant::now() - Duration::from_secs(6));

        // Expired timer should not suppress — session shows in Waiting
        assert_eq!(app.sessions_in_column(Column::Waiting).len(), 1);
        assert_eq!(app.sessions_in_column(Column::Working).len(), 0);
    }

    #[test]
    fn debounce_cancelled_on_working_transition() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "sess", &make_session(Status::Working, 100)).unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();

        // Transition to waiting → starts debounce
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 101)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.process_watcher_events();
        assert!(app.debounce_timers.contains_key("sess"));

        // Transition back to working → cancels debounce
        write_session_to(dir.path(), "sess", &make_session(Status::Working, 102)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.process_watcher_events();
        assert!(!app.debounce_timers.contains_key("sess"));
        assert_eq!(app.sessions_in_column(Column::Working).len(), 1);
    }

    #[test]
    fn debounce_timer_cleanup_on_session_removal() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "sess", &make_session(Status::Working, 100)).unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();

        // Transition to waiting → starts debounce
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 101)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.process_watcher_events();
        assert!(app.debounce_timers.contains_key("sess"));

        // Remove session file → timer should be cleaned up
        std::fs::remove_file(dir.path().join("sess.json")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.process_watcher_events();
        assert!(!app.debounce_timers.contains_key("sess"));
    }

    #[test]
    fn process_debounce_timers_returns_newly_waiting() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 100)).unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();

        // Insert expired timer
        app.debounce_timers
            .insert("sess".to_string(), Instant::now() - Duration::from_secs(6));

        let newly_waiting = app.process_debounce_timers();
        assert_eq!(newly_waiting, vec!["sess"]);
        assert!(!app.debounce_timers.contains_key("sess"));
    }

    #[test]
    fn process_debounce_timers_keeps_active_timers() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 100)).unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();

        // Insert a fresh timer (not expired)
        app.debounce_timers
            .insert("sess".to_string(), Instant::now());

        let newly_waiting = app.process_debounce_timers();
        assert!(newly_waiting.is_empty());
        assert!(app.debounce_timers.contains_key("sess"));
    }

    #[test]
    fn auto_focus_jumps_to_waiting_session() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Waiting, 200)).unwrap();
        write_session_to(dir.path(), "c", &make_session(Status::Waiting, 300)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        // Move focus to Working column
        app.selected_column = app
            .visible_columns()
            .iter()
            .position(|c| *c == Column::Working)
            .unwrap();

        // Auto-focus on the second waiting session
        app.auto_focus_session("c");
        assert_eq!(app.current_column(), Some(Column::Waiting));
        assert_eq!(app.selected_rows.get(&Column::Waiting).copied(), Some(1));
        assert_eq!(app.selected_session(), Some("c"));
    }

    #[test]
    fn debounce_not_restarted_for_already_waiting() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 100)).unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        // No debounce timer — session loaded at startup
        assert!(!app.debounce_timers.contains_key("sess"));

        // Re-write with same status (e.g., new seq) — should NOT start debounce
        write_session_to(dir.path(), "sess", &make_session(Status::Waiting, 101)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.process_watcher_events();

        // Still no debounce timer (session was already waiting)
        assert!(!app.debounce_timers.contains_key("sess"));
        // Shows in Waiting column directly
        assert_eq!(app.sessions_in_column(Column::Waiting).len(), 1);
    }

    // --- Age formatting tests ---

    #[test]
    fn format_age_seconds() {
        assert_eq!(format_age(1000, 1043), "43s");
        assert_eq!(format_age(1000, 1000), "0s");
        assert_eq!(format_age(1000, 1059), "59s");
    }

    #[test]
    fn format_age_minutes() {
        assert_eq!(format_age(1000, 1060), "1m");
        assert_eq!(format_age(1000, 1840), "14m");
        assert_eq!(format_age(1000, 4599), "59m");
    }

    #[test]
    fn format_age_hours() {
        assert_eq!(format_age(1000, 4600), "1h");
        assert_eq!(format_age(1000, 8200), "2h");
        assert_eq!(format_age(0, 360000), "100h");
    }

    #[test]
    fn format_age_future_timestamp_saturates() {
        // If ts is in the future (clock skew), saturating_sub gives 0
        assert_eq!(format_age(2000, 1000), "0s");
    }

    // --- Status icon tests ---

    #[test]
    fn status_icon_waiting() {
        assert_eq!(status_icon(&Status::Waiting), "?");
    }

    #[test]
    fn status_icon_working() {
        assert_eq!(status_icon(&Status::Working), "●");
    }

    #[test]
    fn status_icon_starting_groups_with_working() {
        assert_eq!(status_icon(&Status::Starting), "●");
    }

    #[test]
    fn status_icon_idle() {
        assert_eq!(status_icon(&Status::Idle), "○");
    }

    #[test]
    fn status_icon_done() {
        assert_eq!(status_icon(&Status::Done), "✓");
    }

    // --- Colour scheme tests ---

    #[test]
    fn status_style_waiting_is_yellow() {
        assert_eq!(status_style(&Status::Waiting).fg, Some(Color::Yellow));
    }

    #[test]
    fn status_style_working_is_blue() {
        assert_eq!(status_style(&Status::Working).fg, Some(Color::Blue));
    }

    #[test]
    fn status_style_starting_groups_with_working() {
        assert_eq!(status_style(&Status::Starting).fg, Some(Color::Blue));
    }

    #[test]
    fn status_style_idle_is_dark_gray() {
        assert_eq!(status_style(&Status::Idle).fg, Some(Color::DarkGray));
    }

    #[test]
    fn status_style_done_is_green() {
        assert_eq!(status_style(&Status::Done).fg, Some(Color::Green));
    }

    #[test]
    fn selected_style_has_dark_blue_bg() {
        assert_eq!(selected_style().bg, Some(Color::Blue));
    }

    #[test]
    fn tool_style_is_cyan() {
        assert_eq!(tool_style().fg, Some(Color::Cyan));
    }

    #[test]
    fn dir_style_is_dark_gray() {
        assert_eq!(dir_style().fg, Some(Color::DarkGray));
    }

    #[test]
    fn msg_style_waiting_is_yellow() {
        assert_eq!(msg_style(&Status::Waiting).fg, Some(Color::Yellow));
    }

    #[test]
    fn msg_style_working_is_gray() {
        assert_eq!(msg_style(&Status::Working).fg, Some(Color::Gray));
    }

    #[test]
    fn age_style_is_dark_gray() {
        assert_eq!(age_style().fg, Some(Color::DarkGray));
    }

    #[test]
    fn open_modal_sets_mode_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal("/home/user/project");
        assert_eq!(app.input_mode, InputMode::NewSession);
        assert_eq!(app.modal_name, "");
        assert_eq!(app.modal_dir, "/home/user/project");
        assert_eq!(app.modal_field, 0);
        assert!(app.modal_error.is_none());
    }

    #[test]
    fn close_modal_resets_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal("/tmp");
        app.modal_name = "test".to_string();
        app.close_modal();
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.modal_name, "");
        assert_eq!(app.modal_dir, "");
    }

    #[test]
    fn modal_push_pop_char() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal("/tmp");

        // Field 0 = name
        app.modal_push_char('a');
        app.modal_push_char('b');
        assert_eq!(app.modal_name, "ab");
        app.modal_pop_char();
        assert_eq!(app.modal_name, "a");

        // Switch to field 1 = dir
        app.modal_toggle_field();
        assert_eq!(app.modal_field, 1);
        app.modal_push_char('x');
        assert_eq!(app.modal_dir, "/tmpx");
        app.modal_pop_char();
        assert_eq!(app.modal_dir, "/tmp");
    }

    #[test]
    fn modal_toggle_field_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal("/tmp");
        assert_eq!(app.modal_field, 0);
        app.modal_toggle_field();
        assert_eq!(app.modal_field, 1);
        app.modal_toggle_field();
        assert_eq!(app.modal_field, 0);
    }

    #[test]
    fn validate_modal_rejects_empty_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal(dir.path().to_str().unwrap());
        // Name is empty
        assert!(app.validate_modal().is_err());
        assert!(app.modal_error.as_ref().unwrap().contains("empty"));
    }

    #[test]
    fn validate_modal_rejects_duplicate_name() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "taken", &make_session(Status::Working, 100)).unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal(dir.path().to_str().unwrap());
        app.modal_name = "taken".to_string();
        assert!(app.validate_modal().is_err());
        assert!(app.modal_error.as_ref().unwrap().contains("already exists"));
    }

    #[test]
    fn validate_modal_rejects_bad_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal("/nonexistent/path/12345");
        app.modal_name = "good-name".to_string();
        assert!(app.validate_modal().is_err());
        assert!(app.modal_error.as_ref().unwrap().contains("does not exist"));
    }

    #[test]
    fn validate_modal_accepts_valid_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal(dir.path().to_str().unwrap());
        app.modal_name = "new-sess".to_string();
        let result = app.validate_modal();
        assert!(result.is_ok());
        let (name, resolved_dir) = result.unwrap();
        assert_eq!(name, "new-sess");
        assert_eq!(resolved_dir, dir.path().to_str().unwrap());
    }

    #[test]
    fn expand_tilde_replaces_home() {
        let expanded = App::expand_tilde("~/projects/foo");
        assert!(!expanded.starts_with('~'));
        assert!(expanded.ends_with("/projects/foo"));
    }

    #[test]
    fn expand_tilde_leaves_absolute_path() {
        assert_eq!(App::expand_tilde("/usr/local"), "/usr/local");
    }
}
