// Transcript preview panel rendered in the bottom portion of the screen.

use crate::dashboard::App;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Typed representation of a transcript preview line.
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewLine {
    User(String),
    Assistant(String),
    Tool { name: String, desc: String },
    Separator,
}

/// Render the transcript preview panel into the given area.
///
/// Layout:
///   Line 0: session name (bold white) left, keybinding hints (DarkGray) right
///   Line 1: horizontal separator (`─` repeated, DarkGray)
///   Lines 2+: transcript entries with scroll support
pub fn render_preview(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let w = area.width as usize;

    // Line 0: Header
    let session_name = app.preview_session.as_deref().unwrap_or("?");
    let header_left = format!(" {session_name}");
    let header_right = "\u{2191}\u{2193} scroll  hjkl navigate  Esc close ";
    let header_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let hint_style = Style::default().fg(Color::DarkGray);

    buf.set_string(area.x, area.y, &header_left, header_style);
    if w > header_right.len() {
        let right_x = area.x + (w - header_right.len()) as u16;
        buf.set_string(right_x, area.y, header_right, hint_style);
    }

    if area.height < 3 {
        return;
    }

    // Line 1: Separator
    let sep: String = "\u{2500}".repeat(w);
    buf.set_string(
        area.x,
        area.y + 1,
        &sep,
        Style::default().fg(Color::DarkGray),
    );

    // Lines 2+: Transcript entries
    let body_height = (area.height - 2) as usize;
    let lines = &app.preview_lines;

    if lines.is_empty() {
        let msg = " (transcript not available)";
        buf.set_string(
            area.x,
            area.y + 2,
            msg,
            Style::default().fg(Color::DarkGray),
        );
        return;
    }

    // Scroll: offset 0 = show tail (most recent at bottom)
    let total = lines.len();
    let max_offset = total.saturating_sub(body_height);
    let offset = app.preview_scroll_offset.min(max_offset);
    let start = total.saturating_sub(body_height + offset);
    let end = start + body_height.min(total);

    for (i, line) in lines[start..end].iter().enumerate() {
        let y = area.y + 2 + i as u16;
        if y >= area.y + area.height {
            break;
        }
        render_line(line, area.x, y, w, buf);
    }

    // Scroll indicator: when offset > 0, show "↓↓ more" in bottom-right
    if offset > 0 {
        let indicator = "\u{2193}\u{2193} more";
        let indicator_style = Style::default().fg(Color::Yellow);
        if w > indicator.chars().count() {
            let ix = area.x + (w - indicator.chars().count()) as u16;
            let iy = area.y + area.height - 1;
            buf.set_string(ix, iy, indicator, indicator_style);
        }
    }
}

/// Render a single PreviewLine into the buffer at the given position.
fn render_line(line: &PreviewLine, x: u16, y: u16, max_width: usize, buf: &mut Buffer) {
    match line {
        PreviewLine::User(text) => {
            let label = "User ";
            let label_style = Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD);
            let text_style = Style::default().fg(Color::White);
            buf.set_string(x + 1, y, label, label_style);
            let text_x = x + 1 + label.len() as u16;
            let remaining = max_width.saturating_sub(1 + label.len());
            let display: String = text.chars().take(remaining).collect();
            buf.set_string(text_x, y, &display, text_style);
        }
        PreviewLine::Assistant(text) => {
            let label = "Assistant ";
            let label_style = Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD);
            let text_style = Style::default().fg(Color::Gray);
            buf.set_string(x + 1, y, label, label_style);
            let text_x = x + 1 + label.len() as u16;
            let remaining = max_width.saturating_sub(1 + label.len());
            let display: String = text.chars().take(remaining).collect();
            buf.set_string(text_x, y, &display, text_style);
        }
        PreviewLine::Tool { name, desc } => {
            let label = "Tool ";
            let label_style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD);
            let name_style = Style::default().fg(Color::Cyan);
            let desc_style = Style::default().fg(Color::DarkGray);
            buf.set_string(x + 1, y, label, label_style);
            let mut offset = 1 + label.len();
            let name_display: String = name
                .chars()
                .take(max_width.saturating_sub(offset))
                .collect();
            buf.set_string(x + offset as u16, y, &name_display, name_style);
            offset += name_display.len();
            if !desc.is_empty() && offset < max_width {
                buf.set_string(x + offset as u16, y, " ", desc_style);
                offset += 1;
                let desc_display: String = desc
                    .chars()
                    .take(max_width.saturating_sub(offset))
                    .collect();
                buf.set_string(x + offset as u16, y, &desc_display, desc_style);
            }
        }
        PreviewLine::Separator => {
            // Blank line — nothing rendered
        }
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

    fn cell_fg(buf: &Buffer, x: u16, y: u16) -> Option<Color> {
        buf[(x, y)].style().fg
    }

    #[test]
    fn render_empty_preview_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);
        let text = buffer_text(&buf);
        assert!(text.contains("transcript not available"));
    }

    #[test]
    fn render_preview_with_typed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("test-sess".to_string());
        app.preview_lines = vec![
            PreviewLine::User("hello".to_string()),
            PreviewLine::Assistant("hi there".to_string()),
            PreviewLine::Tool {
                name: "Edit".to_string(),
                desc: "main.rs".to_string(),
            },
        ];

        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("test-sess"));
        assert!(text.contains("hello"));
        assert!(text.contains("hi there"));
        assert!(text.contains("Edit"));
    }

    #[test]
    fn render_user_line_has_yellow_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());
        app.preview_lines = vec![PreviewLine::User("hi".to_string())];

        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        // The "U" of "User " label should be at x=1, y=2 (after header + separator)
        let fg = cell_fg(&buf, 1, 2);
        assert_eq!(fg, Some(Color::Yellow));
    }

    #[test]
    fn render_assistant_line_has_green_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());
        app.preview_lines = vec![PreviewLine::Assistant("hi".to_string())];

        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        let fg = cell_fg(&buf, 1, 2);
        assert_eq!(fg, Some(Color::Green));
    }

    #[test]
    fn render_tool_line_has_cyan_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());
        app.preview_lines = vec![PreviewLine::Tool {
            name: "Edit".to_string(),
            desc: "file.rs".to_string(),
        }];

        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        let fg = cell_fg(&buf, 1, 2);
        assert_eq!(fg, Some(Color::Cyan));
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
        app.preview_lines = vec![PreviewLine::User("a very long message".to_string())];
        let area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);
    }

    #[test]
    fn separator_renders_as_blank_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());
        app.preview_lines = vec![
            PreviewLine::User("first".to_string()),
            PreviewLine::Separator,
            PreviewLine::Assistant("second".to_string()),
        ];

        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("first"));
        assert!(text.contains("second"));
    }

    #[test]
    fn header_shows_keybinding_hints() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());

        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("Esc"));
    }

    #[test]
    fn no_dark_gray_background() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());
        app.preview_lines = vec![PreviewLine::User("test".to_string())];

        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        // Body cell at (1,2) should NOT have DarkGray background
        let bg = buf[(1u16, 2u16)].style().bg;
        assert_ne!(bg, Some(Color::DarkGray));
    }

    #[test]
    fn refresh_preview_builds_typed_lines_with_separators() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();

        // Create a fake transcript file
        let transcript_dir = dir.path().join("transcripts");
        std::fs::create_dir_all(&transcript_dir).unwrap();
        let transcript_path = transcript_dir.join("test.jsonl");
        let content = r#"{"type":"user","message":{"role":"user","content":"hello"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi there"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","id":"x","input":{"file_path":"/a/b/main.rs"}}]}}
{"type":"user","message":{"role":"user","content":"thanks"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"welcome"}]}}
"#;
        std::fs::write(&transcript_path, content).unwrap();

        let session = crate::registry::Session {
            status: crate::registry::Status::Working,
            tool: None,
            desc: None,
            msg: None,
            ts: 1000,
            seq: 1,
            dir: None,
            session_id: None,
            transcript_path: Some(transcript_path.to_string_lossy().to_string()),
            input_tokens: None,
        };
        app.sessions.insert("test-sess".to_string(), session);
        app.preview_session = Some("test-sess".to_string());

        app.refresh_preview();

        // Should have: User, Assistant, Tool, Separator, User, Assistant
        assert!(!app.preview_lines.is_empty());
        let has_separator = app.preview_lines.iter().any(|l| matches!(l, PreviewLine::Separator));
        assert!(has_separator, "should have separator between turns");

        assert!(matches!(&app.preview_lines[0], PreviewLine::User(_)));
        assert!(matches!(&app.preview_lines[1], PreviewLine::Assistant(_)));
        assert!(matches!(&app.preview_lines[2], PreviewLine::Tool { .. }));
        assert!(matches!(&app.preview_lines[3], PreviewLine::Separator));
        assert!(matches!(&app.preview_lines[4], PreviewLine::User(_)));
        assert!(matches!(&app.preview_lines[5], PreviewLine::Assistant(_)));
    }

    #[test]
    fn scroll_offset_affects_visible_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());

        // Create 20 lines
        app.preview_lines = (0..20)
            .map(|i| PreviewLine::Assistant(format!("msg {i}")))
            .collect();

        // With offset 0 and body_height=5 (area height 7 = header+sep+5 body), should show lines 15-19
        let area = Rect::new(0, 0, 80, 7);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);
        let text = buffer_text(&buf);
        assert!(text.contains("msg 19"), "should show last line at offset 0");
        assert!(!text.contains("msg 14"), "should not show line 14 at offset 0");

        // With offset 5, should show lines 10-14
        app.preview_scroll_offset = 5;
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);
        let text = buffer_text(&buf);
        assert!(text.contains("msg 14"), "should show line 14 at offset 5");
        assert!(!text.contains("msg 19"), "should not show last line at offset 5");
    }

    #[test]
    fn scroll_indicator_shown_when_scrolled_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());
        app.preview_lines = (0..20)
            .map(|i| PreviewLine::Assistant(format!("msg {i}")))
            .collect();
        app.preview_scroll_offset = 3;

        let area = Rect::new(0, 0, 80, 7);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);
        let text = buffer_text(&buf);
        assert!(text.contains("more"), "should show scroll indicator");
    }

    #[test]
    fn no_scroll_indicator_at_bottom() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());
        app.preview_lines = (0..20)
            .map(|i| PreviewLine::Assistant(format!("msg {i}")))
            .collect();
        app.preview_scroll_offset = 0;

        let area = Rect::new(0, 0, 80, 7);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);
        let text = buffer_text(&buf);
        assert!(!text.contains("more"), "should not show scroll indicator at bottom");
    }
}
