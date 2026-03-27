// Ratatui app loop, event handling, debounce

use crate::registry::{self, Session, Status};
use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures::StreamExt;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::Terminal;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time;

/// Kanban columns displayed in the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    NeedsAttention,
    Working,
    Done,
}

/// Dashboard input mode — Normal for kanban navigation, NewSession for the modal,
/// Preview for the transcript preview panel.
#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    NewSession,
    Preview,
}

/// Column display order (left to right).
pub const COLUMN_ORDER: [Column; 3] =
    [Column::NeedsAttention, Column::Working, Column::Done];

impl Column {
    /// Column header title.
    pub fn title(self) -> &'static str {
        match self {
            Column::NeedsAttention => "NEEDS ATTENTION",
            Column::Working => "WORKING",
            Column::Done => "DONE",
        }
    }

    /// Map a session status to its kanban column.
    pub fn from_status(status: &Status) -> Column {
        match status {
            Status::Starting | Status::Idle => Column::NeedsAttention,
            Status::Working => Column::Working,
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
    watcher_rx: mpsc::UnboundedReceiver<notify::Result<notify::Event>>,
    /// File watcher handle (must stay alive).
    _watcher: RecommendedWatcher,
    /// Registry directory being watched.
    registry_dir: PathBuf,
    /// Whether the app should quit.
    pub should_quit: bool,
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
    /// Cached preview lines for the transcript panel.
    pub preview_lines: Vec<crate::ui::preview::PreviewLine>,
    /// Name of the session being previewed.
    pub preview_session: Option<String>,
    /// Scroll offset for the preview panel (0 = bottom/most recent).
    pub preview_scroll_offset: usize,
    /// Byte offsets into transcript files for incremental reading.
    pub transcript_offsets: HashMap<String, u64>,
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

        let (tx, rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
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
            preview_lines: Vec::new(),
            preview_session: None,
            preview_scroll_offset: 0,
            transcript_offsets: HashMap::new(),
        };

        // Watch transcript files for sessions that already exist and do initial reads
        let names: Vec<String> = app.sessions.iter()
            .filter(|(_, s)| s.transcript_path.is_some())
            .map(|(name, _)| name.clone())
            .collect();
        for name in &names {
            if let Some(ref path) = app.sessions[name].transcript_path {
                let path = std::path::Path::new(path);
                if path.exists() {
                    let _ = app._watcher.watch(path, notify::RecursiveMode::NonRecursive);
                }
            }
            app.read_transcript(name);
        }

        app.focus_initial_column();
        Ok(app)
    }

    /// Returns all columns in display order.
    pub fn visible_columns(&self) -> Vec<Column> {
        COLUMN_ORDER.to_vec()
    }

    /// Returns (name, session) pairs for a column, sorted by age ascending (oldest first).
    pub fn sessions_in_column(&self, col: Column) -> Vec<(&str, &Session)> {
        let mut entries: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| Column::from_status(&s.status) == col)
            .map(|(name, session)| (name.as_str(), session))
            .collect();
        entries.sort_by_key(|(_, s)| s.ts);
        entries
    }

    /// Reload sessions from registry directory.
    ///
    /// Called when the file watcher detects a registry change.  The triggering
    /// event has already been consumed by `tokio::select!`, so we always
    /// reload — attempting to drain more events first would miss single-event
    /// notifications and could swallow unrelated transcript events from the
    /// shared channel.
    pub fn process_watcher_events(&mut self) {

        let old_sessions = std::mem::take(&mut self.sessions);
        self.sessions = load_sessions_from(&self.registry_dir);

        // Watch transcripts for new sessions and collect names for initial read
        let mut new_transcript_sessions: Vec<String> = Vec::new();
        for (name, session) in &self.sessions {
            if !old_sessions.contains_key(name)
                && let Some(ref path) = session.transcript_path {
                    let path = std::path::Path::new(path);
                    if path.exists() {
                        let _ = self._watcher.watch(path, notify::RecursiveMode::NonRecursive);
                    }
                    new_transcript_sessions.push(name.clone());
                }
        }

        // Unwatch transcripts for removed sessions
        for (name, session) in &old_sessions {
            if !self.sessions.contains_key(name)
                && let Some(ref path) = session.transcript_path {
                    let _ = self._watcher.unwatch(std::path::Path::new(path));
                    self.transcript_offsets.remove(name);
                }
        }

        // Initial transcript read for newly discovered sessions
        for name in &new_transcript_sessions {
            self.read_transcript(name);
        }

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
                .map(|s| Column::from_status(&s.status))
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

    /// Set initial column focus: prefer NeedsAttention (if non-empty), then first non-empty column.
    fn focus_initial_column(&mut self) {
        let visible = self.visible_columns();
        if let Some(idx) = visible
            .iter()
            .position(|c| *c == Column::NeedsAttention && !self.sessions_in_column(*c).is_empty())
        {
            self.selected_column = idx;
        } else if let Some(idx) = visible
            .iter()
            .position(|c| !self.sessions_in_column(*c).is_empty())
        {
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

    /// Move selection to the previous column that has sessions (h key).
    pub fn move_left(&mut self) {
        let visible = self.visible_columns();
        for i in (0..self.selected_column).rev() {
            if let Some(col) = visible.get(i)
                && !self.sessions_in_column(*col).is_empty() {
                    self.selected_column = i;
                    return;
                }
        }
    }

    /// Move selection to the next column that has sessions (l key).
    pub fn move_right(&mut self) {
        let visible = self.visible_columns();
        for i in (self.selected_column + 1)..visible.len() {
            if let Some(col) = visible.get(i)
                && !self.sessions_in_column(*col).is_empty() {
                    self.selected_column = i;
                    return;
                }
        }
    }

    /// Auto-focus: jump selection to the NeedsAttention column and the given session.
    pub fn auto_focus_session(&mut self, name: &str) {
        let visible = self.visible_columns();
        if let Some(idx) = visible.iter().position(|c| *c == Column::NeedsAttention) {
            self.selected_column = idx;
            let entries = self.sessions_in_column(Column::NeedsAttention);
            if let Some(row) = entries.iter().position(|(n, _)| *n == name) {
                self.selected_rows.insert(Column::NeedsAttention, row);
            }
        }
    }

    /// Apply a transcript update to a session's in-memory state.
    /// Does not modify the registry file — transcript state is ephemeral.
    /// Ignores updates for Done sessions (SessionEnd hook is authoritative).
    pub fn apply_transcript_update(&mut self, name: &str, update: crate::transcript::TranscriptUpdate) {
        if let Some(session) = self.sessions.get_mut(name) {
            if session.status == Status::Done {
                return;
            }
            if update.tool.is_some() {
                // Tool use — show the tool name + description
                session.tool = update.tool;
                session.desc = update.desc;
            } else if update.desc.is_some() {
                // Text-only message — Claude is thinking/writing, show the text
                session.tool = None;
                session.desc = update.desc;
            } else if update.status == Status::Idle {
                // End of turn — clear everything
                session.tool = None;
                session.desc = None;
            }
            // Otherwise (no tool, no text, still Working) — preserve previous state
            session.status = update.status;
            if update.input_tokens.is_some() {
                session.input_tokens = update.input_tokens;
            }
            session.ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
        }
    }

    /// Read new bytes from a transcript file and apply any updates.
    /// Returns true if the session state changed.
    pub fn read_transcript(&mut self, name: &str) -> bool {
        let transcript_path = match self.sessions.get(name).and_then(|s| s.transcript_path.as_ref()) {
            Some(p) => p.clone(),
            None => return false,
        };

        let mut file = match std::fs::File::open(&transcript_path) {
            Ok(f) => f,
            Err(_) => return false,
        };

        let offset = self.transcript_offsets.get(name).copied().unwrap_or(0);
        use std::io::{Read, Seek, SeekFrom};
        if file.seek(SeekFrom::Start(offset)).is_err() {
            return false;
        }

        let mut buf = Vec::new();
        let bytes_read = match file.read_to_end(&mut buf) {
            Ok(n) => n,
            Err(_) => return false,
        };

        if bytes_read == 0 {
            return false;
        }

        self.transcript_offsets.insert(name.to_string(), offset + bytes_read as u64);

        if let Some(update) = crate::transcript::parse_new_bytes(&buf) {
            self.apply_transcript_update(name, update);
            true
        } else {
            false
        }
    }

    /// Find which session name corresponds to a transcript file path.
    fn session_for_transcript_path(&self, paths: &[std::path::PathBuf]) -> Option<String> {
        for (name, session) in &self.sessions {
            if let Some(ref tp) = session.transcript_path {
                for path in paths {
                    if path.to_string_lossy() == *tp {
                        return Some(name.clone());
                    }
                }
            }
        }
        None
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

    /// Open the transcript preview for the currently selected session.
    pub fn open_preview(&mut self) {
        if let Some(name) = self.selected_session() {
            self.preview_session = Some(name.to_string());
            self.preview_scroll_offset = 0;
            self.input_mode = InputMode::Preview;
            self.refresh_preview();
        }
    }

    /// Close the transcript preview and return to normal mode.
    pub fn close_preview(&mut self) {
        self.input_mode = InputMode::Normal;
        self.preview_lines.clear();
        self.preview_session = None;
        self.preview_scroll_offset = 0;
    }

    /// Scroll the preview panel up (toward older content).
    pub fn preview_scroll_up(&mut self) {
        self.preview_scroll_offset += 1;
    }

    /// Scroll the preview panel down (toward newer content).
    pub fn preview_scroll_down(&mut self) {
        self.preview_scroll_offset = self.preview_scroll_offset.saturating_sub(1);
    }

    /// Refresh the preview panel by re-reading the transcript tail.
    pub fn refresh_preview(&mut self) {
        if let Some(ref name) = self.preview_session {
            if let Some(session) = self.sessions.get(name)
                && let Some(ref tp) = session.transcript_path
            {
                let path = std::path::Path::new(tp);
                if path.exists() {
                    let entries = crate::transcript::read_tail_all(path, 50);
                    let old_len = self.preview_lines.len();
                    self.preview_lines = build_preview_lines(&entries);
                    if self.preview_scroll_offset > 0 {
                        let new_len = self.preview_lines.len();
                        let delta = new_len.saturating_sub(old_len);
                        self.preview_scroll_offset += delta;
                    }
                    return;
                }
            }
            self.preview_lines = vec![crate::ui::preview::PreviewLine::User(
                "(transcript not available)".to_string(),
            )];
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

/// Run the dashboard TUI event loop.
pub async fn run() -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)
        .context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new()?;
    let result = run_loop(&mut terminal, &mut app).await;

    let _ = disable_raw_mode();
    let _ = crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen);

    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    let mut key_stream = EventStream::new();
    let mut tick = time::interval(Duration::from_secs(1));

    while !app.should_quit {
        terminal.draw(|frame| {
            let area = frame.area();

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            if app.input_mode == InputMode::Preview {
                let chunks =
                    Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)])
                        .split(area);
                crate::ui::kanban::render_kanban(app, chunks[0], frame.buffer_mut(), now);
                crate::ui::preview::render_preview(app, chunks[1], frame.buffer_mut());
            } else {
                let modal_height = if app.input_mode == InputMode::NewSession { 4 } else { 2 };
                let chunks =
                    Layout::vertical([Constraint::Min(0), Constraint::Length(modal_height)])
                        .split(area);
                crate::ui::kanban::render_kanban(app, chunks[0], frame.buffer_mut(), now);
                if app.input_mode == InputMode::NewSession {
                    crate::ui::modal::render_modal(app, chunks[1], frame.buffer_mut());
                } else {
                    crate::ui::statusbar::render_statusbar(app, chunks[1], frame.buffer_mut());
                }
            }
        })?;

        tokio::select! {
            Some(event) = key_stream.next() => {
                if let Ok(Event::Key(key)) = event
                    && key.kind == KeyEventKind::Press {
                        handle_key(app, key.code);
                    }
            }
            Some(event) = app.watcher_rx.recv() => {
                if let Ok(event) = event {
                    let is_transcript = event.paths.iter().any(|p| {
                        p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                    });
                    if is_transcript {
                        if let Some(name) = app.session_for_transcript_path(&event.paths) {
                            let changed = app.read_transcript(&name);
                            if changed
                                && let Some(s) = app.sessions.get(&name)
                                && s.status == Status::Idle {
                                    let name = name.clone();
                                    app.auto_focus_session(&name);
                                }
                        }
                    } else {
                        app.process_watcher_events();
                    }
                }
            }
            _ = tick.tick() => {
                // 1-second tick for age display refresh — triggers redraw
                // Also refresh preview transcript on each tick
                if app.input_mode == InputMode::Preview {
                    app.refresh_preview();
                }
            }
        }
    }

    Ok(())
}

/// Convert transcript entries into preview lines, inserting separators between turns.
fn build_preview_lines(
    entries: &[crate::transcript::TranscriptEntry],
) -> Vec<crate::ui::preview::PreviewLine> {
    use crate::transcript::TranscriptEntry;
    use crate::ui::preview::PreviewLine;

    let mut lines = Vec::new();
    for entry in entries {
        match entry {
            TranscriptEntry::User(text) => {
                if !lines.is_empty() {
                    lines.push(PreviewLine::Separator);
                }
                lines.push(PreviewLine::User(text.clone()));
            }
            TranscriptEntry::Assistant(text) => {
                lines.push(PreviewLine::Assistant(text.clone()));
            }
            TranscriptEntry::Tool(detail) => {
                let (name, desc) = match detail.split_once(' ') {
                    Some((n, d)) => (n.to_string(), d.to_string()),
                    None => (detail.clone(), String::new()),
                };
                lines.push(PreviewLine::Tool { name, desc });
            }
        }
    }
    lines
}

fn handle_key(app: &mut App, code: KeyCode) {
    match app.input_mode {
        InputMode::Normal => match code {
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
            KeyCode::Char('p') => app.open_preview(),
            _ => {}
        },
        InputMode::Preview => if code == KeyCode::Esc { app.close_preview() },
        InputMode::NewSession => match code {
            KeyCode::Esc => app.close_modal(),
            KeyCode::Tab | KeyCode::BackTab => app.modal_toggle_field(),
            KeyCode::Backspace => app.modal_pop_char(),
            KeyCode::Enter => {
                if let Ok((name, dir)) = app.validate_modal() {
                    let env_var = format!("CCMUX_SESSION={name}");
                    let result = crate::zellij::new_tab(
                        &name,
                        "env",
                        &[&env_var, "claude", "--dangerously-skip-permissions", "--worktree"],
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

/// Return the status icon for a session per PRD §8.
/// `?` idle, `●` working/starting, `✓` done.
pub fn status_icon(status: &Status) -> &'static str {
    match status {
        Status::Idle => "?",
        Status::Starting | Status::Working => "●",
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
        Status::Idle => Style::default().fg(Color::Yellow),
        Status::Starting | Status::Working => Style::default().fg(Color::Blue),
        Status::Done => Style::default().fg(Color::Green),
    }
}

/// Style for the selected-card left border.
pub fn selected_style() -> Style {
    Style::default().fg(Color::Blue)
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
        Status::Idle => Style::default().fg(Color::Yellow),
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
            desc: None,
            msg: None,
            ts,
            seq: 0,
            dir: None,
            session_id: None,
            transcript_path: None,
            input_tokens: None,
        }
    }

    #[test]
    fn column_from_status_mapping() {
        assert_eq!(Column::from_status(&Status::Idle), Column::NeedsAttention);
        assert_eq!(Column::from_status(&Status::Starting), Column::NeedsAttention);
        assert_eq!(Column::from_status(&Status::Working), Column::Working);
        assert_eq!(Column::from_status(&Status::Done), Column::Done);
    }

    #[test]
    fn column_titles() {
        assert_eq!(Column::NeedsAttention.title(), "NEEDS ATTENTION");
        assert_eq!(Column::Working.title(), "WORKING");
        assert_eq!(Column::Done.title(), "DONE");
    }

    #[test]
    fn empty_app_shows_all_columns() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.visible_columns().len(), 3);
        assert_eq!(app.current_column(), Some(Column::NeedsAttention));
        assert!(app.selected_session().is_none());
    }

    #[test]
    fn sessions_grouped_into_correct_columns() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Idle, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();
        write_session_to(dir.path(), "d", &make_session(Status::Done, 400)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();

        assert_eq!(app.sessions_in_column(Column::NeedsAttention).len(), 1);
        assert_eq!(app.sessions_in_column(Column::Working).len(), 1);
        assert_eq!(app.sessions_in_column(Column::Done).len(), 1);
    }

    #[test]
    fn visible_columns_always_shows_all() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Idle, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Done, 200)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(
            app.visible_columns(),
            vec![Column::NeedsAttention, Column::Working, Column::Done]
        );
    }

    #[test]
    fn initial_focus_prefers_needs_attention_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Idle, 200)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.current_column(), Some(Column::NeedsAttention));
    }

    #[test]
    fn initial_focus_falls_back_to_first_occupied_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        // No NeedsAttention sessions, so falls back to first occupied column (Working)
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
        write_session_to(dir.path(), "sess", &make_session(Status::Idle, 100)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.selected_session(), Some("sess"));
    }

    #[test]
    fn starting_status_groups_with_needs_attention() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "init", &make_session(Status::Starting, 100)).unwrap();
        write_session_to(dir.path(), "run", &make_session(Status::Working, 200)).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions_in_column(Column::NeedsAttention).len(), 1);
        assert_eq!(app.sessions_in_column(Column::Working).len(), 1);
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
        app.selected_column = 1; // Working column
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
        app.selected_column = 1; // Working column
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
        write_session_to(dir.path(), "a", &make_session(Status::Idle, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        // Initial focus is NeedsAttention (index 0)
        assert_eq!(app.current_column(), Some(Column::NeedsAttention));
        app.move_right();
        assert_eq!(app.current_column(), Some(Column::Working));
    }

    #[test]
    fn move_right_clamps_at_last_occupied_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Idle, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.selected_column = 1; // Working (last occupied column)
        app.move_right(); // should stay — no sessions in Done
        assert_eq!(app.current_column(), Some(Column::Working));
    }

    #[test]
    fn move_left_retreats_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Idle, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Working, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.selected_column = 1; // Working
        assert_eq!(app.current_column(), Some(Column::Working));
        app.move_left();
        assert_eq!(app.current_column(), Some(Column::NeedsAttention));
    }

    #[test]
    fn move_left_clamps_at_first_column() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Idle, 100)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.move_left(); // already at 0
        assert_eq!(app.current_column(), Some(Column::NeedsAttention));
    }

    #[test]
    fn navigation_skips_empty_columns() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Idle, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Done, 200)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.current_column(), Some(Column::NeedsAttention));
        app.move_right();
        // Skips Working (empty), jumps to Done
        assert_eq!(app.current_column(), Some(Column::Done));
        app.move_right();
        // Already at last occupied column
        assert_eq!(app.current_column(), Some(Column::Done));
        app.move_left();
        // Back to NeedsAttention, skipping empty columns
        assert_eq!(app.current_column(), Some(Column::NeedsAttention));
    }

    #[test]
    fn navigation_on_empty_app_stays_put() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        // All columns visible but empty — navigation stays put
        let start = app.selected_column;
        app.move_up();
        app.move_down();
        app.move_left();
        app.move_right();
        assert_eq!(app.selected_column, start);
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

    #[test]
    fn auto_focus_jumps_to_needs_attention_session() {
        let dir = tempfile::tempdir().unwrap();
        write_session_to(dir.path(), "a", &make_session(Status::Working, 100)).unwrap();
        write_session_to(dir.path(), "b", &make_session(Status::Idle, 200)).unwrap();
        write_session_to(dir.path(), "c", &make_session(Status::Idle, 300)).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        // Move focus to Working column
        app.selected_column = app
            .visible_columns()
            .iter()
            .position(|c| *c == Column::Working)
            .unwrap();

        // Auto-focus on the second idle session
        app.auto_focus_session("c");
        assert_eq!(app.current_column(), Some(Column::NeedsAttention));
        assert_eq!(app.selected_rows.get(&Column::NeedsAttention).copied(), Some(1));
        assert_eq!(app.selected_session(), Some("c"));
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
    fn status_icon_idle() {
        assert_eq!(status_icon(&Status::Idle), "?");
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
    fn status_icon_done() {
        assert_eq!(status_icon(&Status::Done), "✓");
    }

    // --- Colour scheme tests ---

    #[test]
    fn status_style_idle_is_yellow() {
        assert_eq!(status_style(&Status::Idle).fg, Some(Color::Yellow));
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
    fn status_style_done_is_green() {
        assert_eq!(status_style(&Status::Done).fg, Some(Color::Green));
    }

    #[test]
    fn selected_style_has_blue_fg() {
        assert_eq!(selected_style().fg, Some(Color::Blue));
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
    fn msg_style_idle_is_yellow() {
        assert_eq!(msg_style(&Status::Idle).fg, Some(Color::Yellow));
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

    #[test]
    fn transcript_update_changes_session_status_and_tokens() {
        let dir = tempfile::tempdir().unwrap();

        let transcript_dir = tempfile::tempdir().unwrap();
        let transcript_path = transcript_dir.path().join("session.jsonl");
        std::fs::write(&transcript_path, "").unwrap();

        let session = Session {
            status: Status::Starting,
            tool: None,
            desc: None,
            msg: None,
            ts: 100,
            seq: 0,
            dir: Some("/project".into()),
            session_id: None,
            transcript_path: Some(transcript_path.to_string_lossy().to_string()),
            input_tokens: None,
        };
        write_session_to(dir.path(), "sess", &session).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();

        let update = crate::transcript::TranscriptUpdate {
            status: Status::Working,
            tool: Some("Bash".into()),
            desc: Some("cargo test".into()),
            input_tokens: Some(34000),
        };
        app.apply_transcript_update("sess", update);

        let s = &app.sessions["sess"];
        assert_eq!(s.status, Status::Working);
        assert_eq!(s.tool.as_deref(), Some("Bash"));
        assert_eq!(s.desc.as_deref(), Some("cargo test"));
        assert_eq!(s.input_tokens, Some(34000));
    }

    #[test]
    fn transcript_update_ignored_for_done_session() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session {
            status: Status::Done,
            tool: None,
            desc: None,
            msg: None,
            ts: 100,
            seq: 0,
            dir: None,
            session_id: None,
            transcript_path: None,
            input_tokens: None,
        };
        write_session_to(dir.path(), "sess", &session).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        let update = crate::transcript::TranscriptUpdate {
            status: Status::Working,
            tool: Some("Edit".into()),
            desc: Some("main.rs".into()),
            input_tokens: Some(5000),
        };
        app.apply_transcript_update("sess", update);

        assert_eq!(app.sessions["sess"].status, Status::Done);
    }

    #[test]
    fn startup_sessions_get_transcript_watched() {
        let dir = tempfile::tempdir().unwrap();
        let transcript_dir = tempfile::tempdir().unwrap();
        let transcript_path = transcript_dir.path().join("session.jsonl");
        std::fs::write(&transcript_path, "").unwrap();

        let session = Session {
            status: Status::Starting,
            tool: None,
            desc: None,
            msg: None,
            ts: 100,
            seq: 0,
            dir: Some("/project".into()),
            session_id: None,
            transcript_path: Some(transcript_path.to_string_lossy().to_string()),
            input_tokens: None,
        };
        write_session_to(dir.path(), "sess", &session).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();

        // Write transcript data
        let transcript_line = r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","id":"x","input":{}}],"usage":{"input_tokens":5000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":50}}}"#;
        std::fs::write(&transcript_path, format!("{}\n", transcript_line)).unwrap();

        // Give the watcher a moment to detect the change
        std::thread::sleep(std::time::Duration::from_millis(200));

        // The transcript change should produce a watcher event
        // Read it manually since we can't use the full async loop in tests
        let changed = app.read_transcript("sess");
        assert!(changed, "transcript should have new data");
        assert_eq!(app.sessions["sess"].status, Status::Working);
        assert_eq!(app.sessions["sess"].tool.as_deref(), Some("Bash"));
        assert_eq!(app.sessions["sess"].input_tokens, Some(5000));
    }
}
