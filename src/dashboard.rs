// Ratatui app loop, event handling, debounce
#![allow(dead_code)]

use crate::registry::{self, Session, Status};
use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

/// Kanban columns displayed in the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    Waiting,
    Working,
    Idle,
    Done,
}

/// Column display order (left to right).
pub const COLUMN_ORDER: [Column; 4] =
    [Column::Waiting, Column::Working, Column::Idle, Column::Done];

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
        };

        app.focus_initial_column();
        Ok(app)
    }

    /// Returns columns that have at least one session, in display order.
    pub fn visible_columns(&self) -> Vec<Column> {
        COLUMN_ORDER
            .iter()
            .copied()
            .filter(|col| !self.sessions_in_column(*col).is_empty())
            .collect()
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

    /// Drain file watcher events and reload sessions if anything changed.
    pub fn process_watcher_events(&mut self) {
        let mut changed = false;
        while self.watcher_rx.try_recv().is_ok() {
            changed = true;
        }
        if changed {
            self.sessions = load_sessions_from(&self.registry_dir);
            self.clamp_selections();
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
}
