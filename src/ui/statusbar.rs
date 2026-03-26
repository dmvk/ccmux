// Bottom status line widget
#![allow(dead_code)]

use crate::dashboard::{status_style, App};
use crate::registry::Status;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

/// Render the two-line status bar into the given area.
///
/// Line 1: session info — `session: <name>  status: <status>  dir: <dir>`
/// Line 2: key bindings — `h/j/k/l navigate · Enter attach · Ctrl+y back · x kill · q quit`
pub fn render_statusbar(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let bar_style = Style::default().fg(Color::DarkGray);

    // Line 1: selected session info
    if area.height >= 1
        && let Some(name) = app.selected_session()
    {
            let session = &app.sessions[name];
            let status_label = status_label(&session.status);
            let dir = session.dir.as_deref().unwrap_or("");

            // Build parts with styling
            let x = area.x + 1;
            let y = area.y;
            let w = area.width.saturating_sub(1) as usize;

            let prefix = "session: ";
            let mid = format!("  status: {status_label}");
            let dir_part = format!("  dir: {dir}");
            let full = format!("{prefix}{name}{mid}{dir_part}");

            // Write the full string, truncated, then overlay colours
            let display = truncate_to(full, w);
            buf.set_string(x, y, &display, bar_style);

            // Overlay session name with status colour
            let name_x = x + prefix.len() as u16;
            let name_style = status_style(&session.status);
            let name_end = std::cmp::min(name.len(), w.saturating_sub(prefix.len()));
            if name_end > 0 {
                buf.set_string(name_x, y, &name[..name_end], name_style);
            }

            // Overlay status label with status colour
            let status_offset = prefix.len() + name.len() + "  status: ".len();
            if status_offset < w {
                let label_end = std::cmp::min(status_label.len(), w.saturating_sub(status_offset));
                if label_end > 0 {
                    buf.set_string(
                        x + status_offset as u16,
                        y,
                        &status_label[..label_end],
                        name_style,
                    );
                }
            }
    }

    // Line 2: key bindings
    if area.height >= 2 {
        let help = "h/j/k/l navigate \u{00b7} Enter attach \u{00b7} n new \u{00b7} x kill \u{00b7} Ctrl+y back \u{00b7} q quit";
        let x = area.x + 1;
        let y = area.y + 1;
        let w = area.width.saturating_sub(1) as usize;
        buf.set_string(x, y, truncate_to(help.to_string(), w), bar_style);
    }
}

/// Human-readable status label for the status bar.
fn status_label(status: &Status) -> &'static str {
    match status {
        Status::Starting => "starting",
        Status::Working => "working",
        Status::Waiting => "waiting",
        Status::Idle => "idle",
        Status::Done => "done",
    }
}

/// Truncate a string to fit within `max` characters.
fn truncate_to(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        s
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{write_session_to, Session, Status};

    fn make_session(status: Status, ts: u64) -> Session {
        Session {
            status,
            tool: None,
            msg: None,
            ts,
            seq: 1,
            dir: Some("~/speedbets/trading".to_string()),
        }
    }

    fn buffer_text(buf: &Buffer) -> String {
        let area = buf.area;
        let mut result = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                let cell = &buf[(x, y)];
                result.push_str(cell.symbol());
            }
            result.push('\n');
        }
        result
    }

    #[test]
    fn render_empty_app_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 80, 2);
        let mut buf = Buffer::empty(area);
        render_statusbar(&app, area, &mut buf);
        // No crash, no selected session → line 1 is blank
    }

    #[test]
    fn render_zero_area_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        render_statusbar(&app, area, &mut buf);
    }

    #[test]
    fn render_selected_session_info() {
        let dir = tempfile::tempdir().unwrap();
        let session = make_session(Status::Waiting, 100);
        write_session_to(dir.path(), "trading", &session).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 80, 2);
        let mut buf = Buffer::empty(area);
        render_statusbar(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("session:"), "session label present");
        assert!(text.contains("trading"), "session name present");
        assert!(text.contains("waiting"), "status label present");
        assert!(text.contains("~/speedbets/trading"), "dir present");
    }

    #[test]
    fn render_keybinding_help() {
        let dir = tempfile::tempdir().unwrap();
        let session = make_session(Status::Working, 100);
        write_session_to(dir.path(), "infra", &session).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 80, 2);
        let mut buf = Buffer::empty(area);
        render_statusbar(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("h/j/k/l"), "navigation keys present");
        assert!(text.contains("Enter attach"), "enter hint present");
        assert!(text.contains("q quit"), "quit hint present");
    }

    #[test]
    fn render_single_line_area_shows_session_only() {
        let dir = tempfile::tempdir().unwrap();
        let session = make_session(Status::Idle, 100);
        write_session_to(dir.path(), "docs", &session).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.selected_column = 2; // Idle column
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_statusbar(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("docs"), "session name on single line");
        assert!(!text.contains("h/j/k/l"), "no help on single line");
    }

    #[test]
    fn render_narrow_area_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let session = make_session(Status::Waiting, 100);
        write_session_to(dir.path(), "trading", &session).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 20, 2);
        let mut buf = Buffer::empty(area);
        render_statusbar(&app, area, &mut buf);
        // Should not panic on narrow terminal
    }

    #[test]
    fn status_label_all_variants() {
        assert_eq!(status_label(&Status::Starting), "starting");
        assert_eq!(status_label(&Status::Working), "working");
        assert_eq!(status_label(&Status::Waiting), "waiting");
        assert_eq!(status_label(&Status::Idle), "idle");
        assert_eq!(status_label(&Status::Done), "done");
    }

    #[test]
    fn render_working_session_shows_working_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = make_session(Status::Working, 100);
        session.tool = Some("Edit".to_string());
        write_session_to(dir.path(), "ml-feats", &session).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.selected_column = 1; // Working column
        let area = Rect::new(0, 0, 80, 2);
        let mut buf = Buffer::empty(area);
        render_statusbar(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("ml-feats"), "session name present");
        assert!(text.contains("working"), "status shows working");
    }

    #[test]
    fn render_session_without_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = make_session(Status::Idle, 100);
        session.dir = None;
        write_session_to(dir.path(), "nodirtest", &session).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.selected_column = 2; // Idle column
        let area = Rect::new(0, 0, 80, 2);
        let mut buf = Buffer::empty(area);
        render_statusbar(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("nodirtest"), "session name present");
        assert!(text.contains("dir:"), "dir label present even without dir");
    }
}
