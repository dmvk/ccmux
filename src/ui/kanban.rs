// Column + card widgets for the kanban dashboard

use crate::dashboard::{
    age_style, dir_style, format_age, msg_style, selected_style, status_icon, status_style,
    tool_style, App, Column,
};
use crate::registry::Status;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};

/// Render the full kanban board into the given area.
///
/// Splits `area` into equal-width columns (with 1-cell vertical separators)
/// for each non-empty column. Each column shows a header, horizontal rule,
/// and session cards.
pub fn render_kanban(app: &App, area: Rect, buf: &mut Buffer, now: u64) {
    let visible = app.visible_columns();
    if visible.is_empty() {
        return;
    }

    // Layout: col | sep | col | sep | ... | col
    let mut constraints = Vec::new();
    for i in 0..visible.len() {
        if i > 0 {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Fill(1));
    }
    let areas = Layout::horizontal(constraints).split(area);
    // areas[0]=col0, areas[1]=sep0, areas[2]=col1, areas[3]=sep1, ...

    for (i, col) in visible.iter().enumerate() {
        let col_area = areas[i * 2];
        let selected_row = if i == app.selected_column {
            Some(app.selected_rows.get(col).copied().unwrap_or(0))
        } else {
            None
        };
        render_column(app, *col, col_area, buf, now, selected_row);

        // Vertical separator between columns
        if i + 1 < visible.len() {
            let sep_area = areas[i * 2 + 1];
            render_column_separator(sep_area, buf, area.y + 1);
        }
    }
}

/// Render a single kanban column: header, horizontal rule, session cards.
fn render_column(
    app: &App,
    col: Column,
    area: Rect,
    buf: &mut Buffer,
    now: u64,
    selected_row: Option<usize>,
) {
    if area.width < 4 || area.height < 3 {
        return;
    }

    let sessions = app.sessions_in_column(col);
    let w = area.width;
    let mut y = area.y;

    // Header: " NEEDS INPUT (2)"
    let header = format!(" {} ({})", col.title(), sessions.len());
    buf.set_string(
        area.x,
        y,
        truncate_str(&header, w as usize),
        header_style(&col),
    );
    y += 1;

    // Horizontal separator
    if y < area.y + area.height {
        let sep: String = "─".repeat(w as usize);
        buf.set_string(area.x, y, &sep, Style::default().fg(Color::DarkGray));
        y += 1;
    }

    // Session cards
    for (i, (name, session)) in sessions.iter().enumerate() {
        if y >= area.y + area.height {
            break;
        }

        let is_sel = selected_row == Some(i);

        // Draw thick left border for selected card
        if is_sel {
            let card_h = std::cmp::min(3, (area.y + area.height).saturating_sub(y));
            let border_style = selected_style();
            for dy in 0..card_h {
                buf.set_string(area.x, y + dy, "▎", border_style);
            }
        }

        // Content offset: leave 1 col for border indicator on selected cards
        let cx = if is_sel { area.x + 1 } else { area.x };
        let cw = if is_sel { w.saturating_sub(1) } else { w };

        // Line 1: icon name  ▰▰▰▱▱▱▱▱ 34k  14m
        {
            let icon = status_icon(&session.status);
            let age = format_age(session.ts, now);
            let age_w = age.chars().count();

            buf.set_string(cx, y, icon, status_style(&session.status));

            // Token bar + count (only if we have token data)
            let token_part = session.input_tokens.map(|tokens| {
                let bar = token_bar(tokens);
                let count = format_tokens(tokens);
                let color = token_bar_color(tokens);
                (bar, count, color)
            });
            let token_w = token_part.as_ref().map_or(0, |(bar, count, _)| bar.chars().count() + 1 + count.len());

            // Name gets remaining space
            let name_avail = (cw as usize).saturating_sub(2 + 1 + token_w + 1 + age_w + 1);
            if name_avail > 0 {
                buf.set_string(
                    cx + 2,
                    y,
                    truncate_str(name, name_avail),
                    status_style(&session.status),
                );
            }

            // Token bar right-aligned before age
            if let Some((bar, count, color)) = token_part {
                let token_str = format!("{} {}", bar, count);
                let token_x = area.x + w - (age_w + 1 + token_str.chars().count()) as u16;
                buf.set_string(token_x, y, &token_str, Style::default().fg(color));
            }

            // Age right-aligned
            let age_x = area.x + w - age_w as u16;
            buf.set_string(age_x, y, &age, age_style());
        }
        y += 1;

        // Line 2: tool (working/starting) or message (waiting)
        if y < area.y + area.height {
            let indent = 2u16;
            let avail = (cw as usize).saturating_sub(indent as usize);
            let lx = cx + indent;

            match session.status {
                Status::Starting | Status::Working => {
                    if let Some(ref tool) = session.tool {
                        let line2 = match session.desc {
                            Some(ref d) => format!("{tool}: {d}"),
                            None => tool.clone(),
                        };
                        buf.set_string(
                            lx,
                            y,
                            truncate_str(&line2, avail),
                            tool_style(),
                        );
                    }
                }
                Status::Idle => {
                    if let Some(ref msg) = session.msg {
                        buf.set_string(
                            lx,
                            y,
                            truncate_str(msg, avail),
                            msg_style(&session.status),
                        );
                    }
                }
                _ => {}
            }
            y += 1;
        }

        // Line 3: directory (shorten $HOME → ~)
        if y < area.y + area.height {
            let indent = 2u16;
            let avail = (cw as usize).saturating_sub(indent as usize);
            if let Some(ref dir) = session.dir {
                let display_dir = shorten_home(dir);
                buf.set_string(
                    cx + indent,
                    y,
                    truncate_str(&display_dir, avail),
                    dir_style(),
                );
            }
            y += 1;
        }

        // Dot separator between cards (not after last)
        if i + 1 < sessions.len() && y < area.y + area.height {
            let dots: String = "·".repeat(w as usize);
            buf.set_string(area.x, y, &dots, Style::default().fg(Color::DarkGray));
            y += 1;
        }
    }
}

/// Render a vertical separator between kanban columns.
/// Uses `┬` at the header separator row and `│` elsewhere.
fn render_column_separator(area: Rect, buf: &mut Buffer, header_sep_y: u16) {
    for y in area.y..area.y + area.height {
        let ch = if y == header_sep_y { "┬" } else { "│" };
        buf.set_string(area.x, y, ch, Style::default().fg(Color::DarkGray));
    }
}

/// Column header style — matches the column's status colour from PRD §8.
fn header_style(col: &Column) -> Style {
    match col {
        Column::NeedsAttention => Style::default().fg(Color::Yellow),
        Column::Working => Style::default().fg(Color::Blue),
        Column::Done => Style::default().fg(Color::Green),
    }
}

const TOKEN_SCALE: u64 = 100_000;
const TOKEN_BAR_WIDTH: u64 = 8;

/// Format a token count as "34k".
fn format_tokens(tokens: u64) -> String {
    format!("{}k", tokens / 1000)
}

/// Generate the token bar string: filled blocks then empty blocks.
fn token_bar(tokens: u64) -> String {
    let clamped = tokens.min(TOKEN_SCALE);
    let filled = (clamped * TOKEN_BAR_WIDTH / TOKEN_SCALE) as usize;
    let empty = (TOKEN_BAR_WIDTH as usize) - filled;
    format!("{}{}", "▰".repeat(filled), "▱".repeat(empty))
}

/// Choose bar color based on token count.
fn token_bar_color(tokens: u64) -> Color {
    if tokens >= 70_000 {
        Color::Red
    } else if tokens >= 40_000 {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// Replace the `$HOME` prefix in a path with `~`.
fn shorten_home(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if path == home.as_ref() {
            return "~".to_string();
        }
        if let Some(rest) = path.strip_prefix(home.as_ref())
            && rest.starts_with('/') {
                return format!("~{rest}");
            }
    }
    path.to_string()
}

/// Truncate a string to fit within `max` display cells, appending ".." if needed.
fn truncate_str(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max {
        return s.to_string();
    }
    if max <= 2 {
        return s.chars().take(max).collect();
    }
    let truncated: String = s.chars().take(max - 2).collect();
    format!("{truncated}..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::App;
    use crate::registry::{write_session_to, Session, Status};

    fn make_session(status: Status, ts: u64) -> Session {
        Session {
            status,
            tool: None,
            desc: None,
            msg: None,
            ts,
            seq: 1,
            dir: Some("~/project".to_string()),
            session_id: None,
            transcript_path: None,
            input_tokens: None,
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
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        render_kanban(&app, area, &mut buf, 1000);
    }

    #[test]
    fn render_single_idle_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = make_session(Status::Idle, 950);
        session.msg = Some("Should I proceed?".to_string());
        write_session_to(dir.path(), "trading", &session).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 120, 10);
        let mut buf = Buffer::empty(area);
        render_kanban(&app, area, &mut buf, 1000);

        let text = buffer_text(&buf);
        assert!(text.contains("NEEDS ATTENTION"), "header present");
        assert!(text.contains("trading"), "session name present");
        assert!(text.contains("50s"), "age present");
        assert!(text.contains("Should I proceed?"), "message present");
        assert!(text.contains("~/project"), "directory present");
    }

    #[test]
    fn render_multiple_columns() {
        let dir = tempfile::tempdir().unwrap();
        let mut s1 = make_session(Status::Idle, 950);
        s1.msg = Some("Proceed?".to_string());
        write_session_to(dir.path(), "alpha", &s1).unwrap();

        let mut s2 = make_session(Status::Working, 980);
        s2.tool = Some("Edit".to_string());
        write_session_to(dir.path(), "beta", &s2).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 80, 12);
        let mut buf = Buffer::empty(area);
        render_kanban(&app, area, &mut buf, 1000);

        let text = buffer_text(&buf);
        assert!(text.contains("NEEDS ATTENTION"), "needs attention header");
        assert!(text.contains("WORKING"), "working header");
        assert!(text.contains("alpha"), "idle session");
        assert!(text.contains("beta"), "working session");
        assert!(text.contains("Edit"), "tool name");
    }

    #[test]
    fn render_narrow_area_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let session = make_session(Status::Working, 990);
        write_session_to(dir.path(), "tiny", &session).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        render_kanban(&app, area, &mut buf, 1000);
    }

    #[test]
    fn render_selected_card_has_content() {
        let dir = tempfile::tempdir().unwrap();
        let session = make_session(Status::Idle, 950);
        write_session_to(dir.path(), "selected", &session).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 120, 10);
        let mut buf = Buffer::empty(area);
        render_kanban(&app, area, &mut buf, 1000);

        let text = buffer_text(&buf);
        assert!(text.contains("selected"), "selected session name present");
    }

    #[test]
    fn column_separator_renders() {
        let area = Rect::new(5, 0, 1, 4);
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 4));
        render_column_separator(area, &mut buf, 1);
        assert_eq!(buf[(5, 0)].symbol(), "│");
        assert_eq!(buf[(5, 1)].symbol(), "┬");
        assert_eq!(buf[(5, 2)].symbol(), "│");
        assert_eq!(buf[(5, 3)].symbol(), "│");
    }

    #[test]
    fn truncate_no_change() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_fit() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn truncate_with_dots() {
        assert_eq!(truncate_str("hello world", 7), "hello..");
    }

    #[test]
    fn truncate_very_short_max() {
        assert_eq!(truncate_str("hello", 2), "he");
    }

    #[test]
    fn truncate_zero_max() {
        assert_eq!(truncate_str("hello", 0), "");
    }

    #[test]
    fn render_card_shows_tool_with_desc() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = make_session(Status::Working, 980);
        session.tool = Some("Bash".to_string());
        session.desc = Some("Install dependencies".to_string());
        write_session_to(dir.path(), "builder", &session).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 120, 10);
        let mut buf = Buffer::empty(area);
        render_kanban(&app, area, &mut buf, 1000);

        let text = buffer_text(&buf);
        assert!(
            text.contains("Bash: Install dependencies"),
            "should show 'Tool: desc', got:\n{text}"
        );
    }

    #[test]
    fn render_card_shows_tool_only_without_desc() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = make_session(Status::Working, 980);
        session.tool = Some("Bash".to_string());
        // desc is None (default from make_session)
        write_session_to(dir.path(), "runner", &session).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 120, 10);
        let mut buf = Buffer::empty(area);
        render_kanban(&app, area, &mut buf, 1000);

        let text = buffer_text(&buf);
        assert!(text.contains("Bash"), "should still show tool name");
        assert!(
            !text.contains("Bash:"),
            "should NOT show colon when no desc"
        );
    }

    #[test]
    fn render_dot_separator_between_cards() {
        let dir = tempfile::tempdir().unwrap();
        let s1 = make_session(Status::Idle, 900);
        let s2 = make_session(Status::Idle, 950);
        write_session_to(dir.path(), "aaa", &s1).unwrap();
        write_session_to(dir.path(), "bbb", &s2).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 40, 14);
        let mut buf = Buffer::empty(area);
        render_kanban(&app, area, &mut buf, 1000);

        let text = buffer_text(&buf);
        assert!(text.contains('·'), "dot separator between cards");
    }

    #[test]
    fn format_tokens_display() {
        assert_eq!(format_tokens(0), "0k");
        assert_eq!(format_tokens(500), "0k");
        assert_eq!(format_tokens(1000), "1k");
        assert_eq!(format_tokens(34000), "34k");
        assert_eq!(format_tokens(34500), "34k");
        assert_eq!(format_tokens(100000), "100k");
        assert_eq!(format_tokens(150000), "150k");
    }

    #[test]
    fn token_bar_string_generation() {
        assert_eq!(token_bar(0), "▱▱▱▱▱▱▱▱");
        assert_eq!(token_bar(12500), "▰▱▱▱▱▱▱▱");
        assert_eq!(token_bar(50000), "▰▰▰▰▱▱▱▱");
        assert_eq!(token_bar(100000), "▰▰▰▰▰▰▰▰");
        assert_eq!(token_bar(150000), "▰▰▰▰▰▰▰▰");
    }

    #[test]
    fn token_bar_color_thresholds() {
        assert_eq!(token_bar_color(0), Color::Green);
        assert_eq!(token_bar_color(39000), Color::Green);
        assert_eq!(token_bar_color(40000), Color::Yellow);
        assert_eq!(token_bar_color(69000), Color::Yellow);
        assert_eq!(token_bar_color(70000), Color::Red);
        assert_eq!(token_bar_color(150000), Color::Red);
    }

    #[test]
    fn render_card_with_token_data() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = make_session(Status::Working, 950);
        session.tool = Some("Edit".to_string());
        session.input_tokens = Some(34000);
        write_session_to(dir.path(), "trading", &session).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        // Use 120 wide so each of the 3 columns gets ~39 chars — enough for
        // name + token bar + age to all appear simultaneously.
        let area = Rect::new(0, 0, 120, 10);
        let mut buf = Buffer::empty(area);
        render_kanban(&app, area, &mut buf, 1000);

        let text = buffer_text(&buf);
        assert!(text.contains("▰▰"), "token bar present");
        assert!(text.contains("34k"), "token count present");
        assert!(text.contains("trading"), "session name present");
    }

    #[test]
    fn render_card_without_token_data() {
        let dir = tempfile::tempdir().unwrap();
        let session = make_session(Status::Starting, 990);
        write_session_to(dir.path(), "fresh", &session).unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        render_kanban(&app, area, &mut buf, 1000);

        let text = buffer_text(&buf);
        assert!(text.contains("fresh"), "session name present");
        assert!(!text.contains("▰"), "no token bar when no data");
        assert!(!text.contains("▱"), "no token bar when no data");
    }
}
