// New-session modal widget rendered over the status bar area.

use crate::dashboard::App;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Render the new-session modal into the given area (replaces status bar).
///
/// Layout (4 lines):
///   Line 0: "New Session                        Esc cancel · Tab switch · Enter create"
///   Line 1: "Name: <name input>                 "
///   Line 2: "Dir:  <dir input>                  "
///   Line 3: "<error message if any>             "
pub fn render_modal(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let w = area.width as usize;
    let bg = Style::default().bg(Color::DarkGray).fg(Color::White);
    let active_bg = Style::default().bg(Color::Blue).fg(Color::White);
    let error_style = Style::default().fg(Color::Red);
    let label_style = bg.add_modifier(Modifier::BOLD);

    // Clear the area with background
    let blank: String = " ".repeat(w);
    for dy in 0..area.height {
        buf.set_string(area.x, area.y + dy, &blank, bg);
    }

    let x = area.x + 1;
    let avail = w.saturating_sub(2);

    // Line 0: Title + help
    if area.height >= 1 {
        let title = "New Session";
        let help = "Esc cancel \u{00b7} Tab switch \u{00b7} Enter create";
        buf.set_string(x, area.y, title, label_style);
        let help_x = (area.x + area.width).saturating_sub(help.len() as u16 + 1);
        if help_x > x + title.len() as u16 + 1 {
            buf.set_string(help_x, area.y, help, bg);
        }
    }

    // Line 1: Name field
    if area.height >= 2 {
        let label = "Name: ";
        let field_bg = if app.modal_field == 0 { active_bg } else { bg };
        buf.set_string(x, area.y + 1, label, bg);
        let input_x = x + label.len() as u16;
        let input_avail = avail.saturating_sub(label.len());
        let display = truncate_to(&app.modal_name, input_avail);
        buf.set_string(input_x, area.y + 1, &display, field_bg);
        // Cursor indicator
        if app.modal_field == 0 {
            let cursor_x = input_x + display.len() as u16;
            if (cursor_x - area.x) < area.width {
                buf.set_string(cursor_x, area.y + 1, "_", field_bg);
            }
        }
    }

    // Line 2: Directory field
    if area.height >= 3 {
        let label = "Dir:  ";
        let field_bg = if app.modal_field == 1 { active_bg } else { bg };
        buf.set_string(x, area.y + 2, label, bg);
        let input_x = x + label.len() as u16;
        let input_avail = avail.saturating_sub(label.len());
        let display = truncate_to(&app.modal_dir, input_avail);
        buf.set_string(input_x, area.y + 2, &display, field_bg);
        // Cursor indicator
        if app.modal_field == 1 {
            let cursor_x = input_x + display.len() as u16;
            if (cursor_x - area.x) < area.width {
                buf.set_string(cursor_x, area.y + 2, "_", field_bg);
            }
        }
    }

    // Line 3: Error message
    if area.height >= 4
        && let Some(ref err) = app.modal_error
    {
        let display = truncate_to(err, avail);
        buf.set_string(x, area.y + 3, &display, error_style);
    }
}

/// Truncate a string to fit within `max` characters.
fn truncate_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::App;

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
    fn render_modal_shows_title_and_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal("/home/user");
        app.modal_name = "my-sess".to_string();

        let area = Rect::new(0, 0, 80, 4);
        let mut buf = Buffer::empty(area);
        render_modal(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("New Session"), "title present");
        assert!(text.contains("Name:"), "name label present");
        assert!(text.contains("my-sess"), "name value present");
        assert!(text.contains("Dir:"), "dir label present");
        assert!(text.contains("/home/user"), "dir value present");
    }

    #[test]
    fn render_modal_shows_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal("/tmp");
        app.modal_error = Some("session name cannot be empty".to_string());

        let area = Rect::new(0, 0, 80, 4);
        let mut buf = Buffer::empty(area);
        render_modal(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("cannot be empty"), "error message present");
    }

    #[test]
    fn render_modal_shows_help_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal("/tmp");

        let area = Rect::new(0, 0, 80, 4);
        let mut buf = Buffer::empty(area);
        render_modal(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("Esc cancel"), "escape hint present");
        assert!(text.contains("Tab switch"), "tab hint present");
        assert!(text.contains("Enter create"), "enter hint present");
    }

    #[test]
    fn render_modal_zero_area_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal("/tmp");

        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        render_modal(&app, area, &mut buf);
    }

    #[test]
    fn render_modal_narrow_area_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.open_new_session_modal("/tmp");
        app.modal_name = "a-very-long-session-name".to_string();

        let area = Rect::new(0, 0, 15, 4);
        let mut buf = Buffer::empty(area);
        render_modal(&app, area, &mut buf);
        // Just ensure no panic
    }
}
