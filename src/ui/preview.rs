// Transcript preview panel rendered in the bottom portion of the screen.

use crate::dashboard::App;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Render the transcript preview panel into the given area.
///
/// Layout:
///   Line 0: "Preview: <session_name>          Esc close"
///   Line 1: horizontal separator
///   Lines 2+: transcript entries (tail, most recent at bottom)
pub fn render_preview(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let w = area.width as usize;
    let bg = Style::default().bg(Color::DarkGray).fg(Color::White);

    // Clear the area with background
    let blank: String = " ".repeat(w);
    for dy in 0..area.height {
        buf.set_string(area.x, area.y + dy, &blank, bg);
    }

    // Line 0: Header
    let session_name = app.preview_session.as_deref().unwrap_or("?");
    let header_left = format!(" Preview: {session_name}");
    let header_right = "Esc close ";
    let header_style = bg.add_modifier(Modifier::BOLD);

    buf.set_string(area.x, area.y, &header_left, header_style);
    if w > header_right.len() {
        let right_x = area.x + (w - header_right.len()) as u16;
        buf.set_string(right_x, area.y, header_right, Style::default().bg(Color::DarkGray).fg(Color::DarkGray));
    }

    if area.height < 3 {
        return;
    }

    // Line 1: Separator
    let sep: String = "\u{2500}".repeat(w);
    buf.set_string(area.x, area.y + 1, &sep, Style::default().bg(Color::DarkGray).fg(Color::Gray));

    // Lines 2+: Transcript entries
    let body_height = (area.height - 2) as usize;
    let lines = &app.preview_lines;

    if lines.is_empty() {
        let msg = " (transcript not available)";
        buf.set_string(
            area.x,
            area.y + 2,
            msg,
            Style::default().bg(Color::DarkGray).fg(Color::Gray),
        );
        return;
    }

    // Show the tail that fits
    let start = lines.len().saturating_sub(body_height);
    for (i, line) in lines[start..].iter().enumerate() {
        let y = area.y + 2 + i as u16;
        if y >= area.y + area.height {
            break;
        }

        let style = entry_style(line);
        // Truncate to width
        let display = if line.chars().count() > w.saturating_sub(1) {
            let truncated: String = line.chars().take(w.saturating_sub(1)).collect();
            format!(" {truncated}")
        } else {
            format!(" {line}")
        };
        buf.set_string(area.x, y, &display, style);
    }
}

/// Determine the style for a preview line based on its prefix.
fn entry_style(line: &str) -> Style {
    let bg = Color::DarkGray;
    if line.starts_with("User:") {
        Style::default().bg(bg).fg(Color::White).add_modifier(Modifier::BOLD)
    } else if line.starts_with("Assistant:") {
        Style::default().bg(bg).fg(Color::Gray)
    } else if line.starts_with("Tool:") {
        Style::default().bg(bg).fg(Color::Cyan)
    } else {
        Style::default().bg(bg).fg(Color::Gray)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::App;
    use ratatui::buffer::Buffer;

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
    fn render_empty_preview_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);
        let text = buffer_text(&buf);
        assert!(text.contains("Preview:"));
        assert!(text.contains("transcript not available"));
    }

    #[test]
    fn render_preview_with_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("test-sess".to_string());
        app.preview_lines = vec![
            "User: hello".to_string(),
            "Assistant: hi there".to_string(),
            "Tool: Edit main.rs".to_string(),
        ];

        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("test-sess"));
        assert!(text.contains("User: hello"));
        assert!(text.contains("Assistant: hi there"));
        assert!(text.contains("Tool: Edit main.rs"));
    }

    #[test]
    fn render_zero_area_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        render_preview(&app, area, &mut buf);
    }

    #[test]
    fn render_narrow_area_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_lines = vec!["User: a very long message".to_string()];
        let area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);
    }

    #[test]
    fn entry_style_user_is_bold_white() {
        let style = entry_style("User: hello");
        assert_eq!(style.fg, Some(Color::White));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn entry_style_assistant_is_gray() {
        let style = entry_style("Assistant: hello");
        assert_eq!(style.fg, Some(Color::Gray));
    }

    #[test]
    fn entry_style_tool_is_cyan() {
        let style = entry_style("Tool: Edit");
        assert_eq!(style.fg, Some(Color::Cyan));
    }
}
