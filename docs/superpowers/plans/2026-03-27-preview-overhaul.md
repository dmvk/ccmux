# Preview Panel Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign the transcript preview panel with no background, rich colors, card navigation, scroll support, and watcher-driven live updates.

**Architecture:** Replace the flat `Vec<String>` preview model with a typed `PreviewLine` enum rendered with per-segment coloring. Add h/j/k/l card navigation and Up/Down scroll in Preview mode. Switch from 1-second tick to file-watcher-driven refresh.

**Tech Stack:** Rust, ratatui, crossterm, notify (file watcher)

---

## File Map

| File | Role | Action |
|------|------|--------|
| `src/ui/preview.rs` | Preview panel rendering | Rewrite: new `PreviewLine` enum, new `render_preview`, new `render_line`, scroll indicator |
| `src/dashboard.rs` | App state + event loop + input handling | Modify: new field `preview_scroll_offset`, update `refresh_preview`, update `open_preview`/`close_preview`, new Preview-mode key handlers, watcher-driven preview refresh |
| `src/transcript.rs` | Transcript parsing | No changes needed — `TranscriptEntry` already carries the data; `format_entry` stays for backward compat |

---

### Task 1: Add `PreviewLine` enum and render scaffolding

**Files:**
- Modify: `src/ui/preview.rs`

- [ ] **Step 1: Write failing test for PreviewLine rendering**

Add a test that constructs `PreviewLine` variants and renders them. This replaces the old `render_preview_with_lines` test.

```rust
// In src/ui/preview.rs, replace the existing tests module entirely with:

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
            PreviewLine::User("hello".into()),
            PreviewLine::Assistant("hi there".into()),
            PreviewLine::Tool {
                name: "Edit".into(),
                desc: "main.rs".into(),
            },
        ];

        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("test-sess"));
        assert!(text.contains("User"));
        assert!(text.contains("hello"));
        assert!(text.contains("Assistant"));
        assert!(text.contains("hi there"));
        assert!(text.contains("Edit"));
        assert!(text.contains("main.rs"));
    }

    #[test]
    fn render_user_line_has_yellow_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());
        app.preview_lines = vec![PreviewLine::User("hi".into())];

        let area = Rect::new(0, 0, 80, 5);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        // "User" label starts at x=1 (1-char left padding), row 2 (after header+sep)
        // Check that the 'U' of "User" is yellow
        assert_eq!(cell_fg(&buf, 1, 2), Some(Color::Yellow));
    }

    #[test]
    fn render_assistant_line_has_green_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());
        app.preview_lines = vec![PreviewLine::Assistant("ok".into())];

        let area = Rect::new(0, 0, 80, 5);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        assert_eq!(cell_fg(&buf, 1, 2), Some(Color::Green));
    }

    #[test]
    fn render_tool_line_has_cyan_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.preview_session = Some("s".to_string());
        app.preview_lines = vec![PreviewLine::Tool {
            name: "Bash".into(),
            desc: "ls".into(),
        }];

        let area = Rect::new(0, 0, 80, 5);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        assert_eq!(cell_fg(&buf, 1, 2), Some(Color::Cyan));
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
        app.preview_lines = vec![PreviewLine::User("a very long message".into())];
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
            PreviewLine::Assistant("first".into()),
            PreviewLine::Separator,
            PreviewLine::User("second".into()),
        ];

        let area = Rect::new(0, 0, 80, 8);
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
        app.preview_session = Some("mysess".to_string());

        let area = Rect::new(0, 0, 80, 5);
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
        app.preview_lines = vec![PreviewLine::User("hi".into())];

        let area = Rect::new(0, 0, 80, 5);
        let mut buf = Buffer::empty(area);
        render_preview(&app, area, &mut buf);

        // Check that body cells do NOT have DarkGray background
        let cell_style = buf[(1, 2)].style();
        assert_ne!(cell_style.bg, Some(Color::DarkGray));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ccmux -- preview::tests --no-capture 2>&1 | head -50`
Expected: compilation errors because `PreviewLine` doesn't exist yet and `preview_lines` is still `Vec<String>`

- [ ] **Step 3: Define `PreviewLine` enum and update `App` fields**

In `src/ui/preview.rs`, add the enum at the top (after imports):

```rust
/// A typed line for rendering in the preview panel.
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewLine {
    User(String),
    Assistant(String),
    Tool { name: String, desc: String },
    Separator,
}
```

In `src/dashboard.rs`, change the `preview_lines` field and add `preview_scroll_offset`:

```rust
// Old:
pub preview_lines: Vec<String>,
// New:
pub preview_lines: Vec<crate::ui::preview::PreviewLine>,
/// Scroll offset for preview panel (lines scrolled up from bottom, 0 = auto-tail).
pub preview_scroll_offset: usize,
```

In `App::with_registry_dir` initializer, add:

```rust
preview_scroll_offset: 0,
```

Update `open_preview` to reset scroll:

```rust
pub fn open_preview(&mut self) {
    if let Some(name) = self.selected_session() {
        self.preview_session = Some(name.to_string());
        self.preview_scroll_offset = 0;
        self.input_mode = InputMode::Preview;
        self.refresh_preview();
    }
}
```

Update `close_preview` to clear scroll:

```rust
pub fn close_preview(&mut self) {
    self.input_mode = InputMode::Normal;
    self.preview_lines.clear();
    self.preview_session = None;
    self.preview_scroll_offset = 0;
}
```

- [ ] **Step 4: Rewrite `render_preview` with new styling**

Replace the entire `render_preview` function and `entry_style` in `src/ui/preview.rs`:

```rust
use crate::dashboard::App;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// A typed line for rendering in the preview panel.
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
///   Line 0: " session_name         ^/v scroll  h/j/k/l navigate  Esc close"
///   Line 1: horizontal separator (─)
///   Lines 2+: transcript entries with scroll offset support
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

    // Scroll indicator when not at bottom
    if offset > 0 {
        let indicator = "\u{2193}\u{2193} more";
        if w > indicator.len() + 1 {
            let ix = area.x + (w - indicator.len() - 1) as u16;
            let iy = area.y + area.height - 1;
            buf.set_string(ix, iy, indicator, Style::default().fg(Color::Yellow));
        }
    }
}

/// Render a single PreviewLine at the given position.
fn render_line(line: &PreviewLine, x: u16, y: u16, max_w: usize, buf: &mut Buffer) {
    let pad = 1u16; // left padding
    let content_w = max_w.saturating_sub(pad as usize);
    let cx = x + pad;

    match line {
        PreviewLine::User(text) => {
            let label = "User ";
            let label_style = Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD);
            let text_style = Style::default().fg(Color::White);
            buf.set_string(cx, y, label, label_style);
            let remaining = content_w.saturating_sub(label.len());
            let display: String = text.chars().take(remaining).collect();
            buf.set_string(cx + label.len() as u16, y, &display, text_style);
        }
        PreviewLine::Assistant(text) => {
            let label = "Assistant ";
            let label_style = Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD);
            let text_style = Style::default().fg(Color::Gray);
            buf.set_string(cx, y, label, label_style);
            let remaining = content_w.saturating_sub(label.len());
            let display: String = text.chars().take(remaining).collect();
            buf.set_string(cx + label.len() as u16, y, &display, text_style);
        }
        PreviewLine::Tool { name, desc } => {
            let label = "Tool ";
            let label_style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD);
            let name_style = Style::default().fg(Color::Cyan);
            let desc_style = Style::default().fg(Color::DarkGray);
            buf.set_string(cx, y, label, label_style);
            let mut offset = label.len();
            let name_display: String = name.chars().take(content_w.saturating_sub(offset)).collect();
            buf.set_string(cx + offset as u16, y, &name_display, name_style);
            offset += name_display.len();
            if !desc.is_empty() && offset < content_w {
                let space = " ";
                buf.set_string(cx + offset as u16, y, space, desc_style);
                offset += 1;
                let desc_display: String = desc.chars().take(content_w.saturating_sub(offset)).collect();
                buf.set_string(cx + offset as u16, y, &desc_display, desc_style);
            }
        }
        PreviewLine::Separator => {
            // Blank line — nothing to render
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ccmux -- preview::tests --no-capture`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/ui/preview.rs src/dashboard.rs
git commit -m "feat: add PreviewLine enum and rewrite preview renderer with rich colors"
```

---

### Task 2: Update `refresh_preview` to build `PreviewLine` values with turn separators

**Files:**
- Modify: `src/dashboard.rs:479-497` (the `refresh_preview` method)

- [ ] **Step 1: Write failing test for PreviewLine construction with separators**

Add to the existing test module in `src/dashboard.rs` (or the preview tests — wherever `refresh_preview` is testable). Since `refresh_preview` reads from a real file, write an integration-style test:

```rust
// In src/ui/preview.rs tests module, add:

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

    // Register a session with this transcript
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

    // Check separator exists before second User entry
    let has_separator = app.preview_lines.iter().any(|l| matches!(l, PreviewLine::Separator));
    assert!(has_separator, "should have separator between turns");

    // Check types
    assert!(matches!(&app.preview_lines[0], PreviewLine::User(_)));
    assert!(matches!(&app.preview_lines[1], PreviewLine::Assistant(_)));
    assert!(matches!(&app.preview_lines[2], PreviewLine::Tool { .. }));
    assert!(matches!(&app.preview_lines[3], PreviewLine::Separator));
    assert!(matches!(&app.preview_lines[4], PreviewLine::User(_)));
    assert!(matches!(&app.preview_lines[5], PreviewLine::Assistant(_)));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ccmux -- refresh_preview_builds_typed_lines --no-capture`
Expected: FAIL — `refresh_preview` still builds `Vec<String>` via `format_entry`

- [ ] **Step 3: Update `refresh_preview` to build `PreviewLine` values**

In `src/dashboard.rs`, replace the `refresh_preview` method:

```rust
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
                // Scroll stability: if scrolled up, adjust offset by delta
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
```

Add this free function in `src/dashboard.rs` (below `refresh_preview` or near the bottom, before `run_loop`):

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ccmux -- preview --no-capture`
Expected: all preview tests pass

- [ ] **Step 5: Commit**

```bash
git add src/dashboard.rs
git commit -m "feat: build typed PreviewLine values with turn separators in refresh_preview"
```

---

### Task 3: Add scroll tests and scroll methods

**Files:**
- Modify: `src/ui/preview.rs` (add scroll tests)
- Modify: `src/dashboard.rs` (add scroll methods)

Note: `preview_scroll_offset` field was already added in Task 1. This task adds the scroll methods and tests.

- [ ] **Step 1: Write scroll tests**

Add to `src/ui/preview.rs` tests:

```rust
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
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p ccmux -- scroll --no-capture`
Expected: PASS (the renderer and field already support scroll from Task 1)

- [ ] **Step 3: Add scroll methods to App**

In `src/dashboard.rs`, add these methods to `App`:

```rust
/// Scroll the preview panel up (toward older content).
pub fn preview_scroll_up(&mut self) {
    self.preview_scroll_offset += 1;
}

/// Scroll the preview panel down (toward newer content).
pub fn preview_scroll_down(&mut self) {
    self.preview_scroll_offset = self.preview_scroll_offset.saturating_sub(1);
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ccmux -- preview --no-capture`
Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
git add src/dashboard.rs src/ui/preview.rs
git commit -m "feat: add scroll tests and scroll up/down methods"
```

---

### Task 4: Add h/j/k/l and Up/Down key handlers in Preview mode

**Files:**
- Modify: `src/dashboard.rs:683` (the `InputMode::Preview` match arm in `handle_key`)

- [ ] **Step 1: Write failing test for Preview-mode key handling**

Add to the existing dashboard test module (find where `handle_key` tests live, or add to the bottom of dashboard tests):

```rust
#[cfg(test)]
mod preview_key_tests {
    use super::*;
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

    #[test]
    fn preview_hjkl_navigates_cards() {
        let dir = tempfile::tempdir().unwrap();
        let mut s1 = make_session(Status::Idle, 950);
        s1.msg = Some("q1".to_string());
        write_session_to(dir.path(), "alpha", &s1).unwrap();
        let mut s2 = make_session(Status::Working, 980);
        s2.tool = Some("Edit".to_string());
        write_session_to(dir.path(), "beta", &s2).unwrap();

        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.input_mode = InputMode::Preview;
        app.preview_session = Some("alpha".to_string());

        // Navigate right (l key) — should move to Working column
        handle_key(&mut app, KeyCode::Char('l'));
        assert_eq!(app.preview_session.as_deref(), Some("beta"));
        assert_eq!(app.preview_scroll_offset, 0);
    }

    #[test]
    fn preview_up_down_scrolls() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.input_mode = InputMode::Preview;
        app.preview_session = Some("s".to_string());
        app.preview_scroll_offset = 0;

        handle_key(&mut app, KeyCode::Up);
        assert_eq!(app.preview_scroll_offset, 1);

        handle_key(&mut app, KeyCode::Up);
        assert_eq!(app.preview_scroll_offset, 2);

        handle_key(&mut app, KeyCode::Down);
        assert_eq!(app.preview_scroll_offset, 1);

        handle_key(&mut app, KeyCode::Down);
        assert_eq!(app.preview_scroll_offset, 0);

        // Can't go below 0
        handle_key(&mut app, KeyCode::Down);
        assert_eq!(app.preview_scroll_offset, 0);
    }

    #[test]
    fn preview_esc_closes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_registry_dir(dir.path()).unwrap();
        app.input_mode = InputMode::Preview;
        app.preview_session = Some("s".to_string());

        handle_key(&mut app, KeyCode::Esc);
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.preview_session.is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ccmux -- preview_key_tests --no-capture`
Expected: FAIL — Preview mode only handles Esc currently

- [ ] **Step 3: Expand Preview mode key handler**

In `src/dashboard.rs`, replace line 683:

```rust
// Old:
InputMode::Preview => if code == KeyCode::Esc { app.close_preview() },
```

With:

```rust
InputMode::Preview => match code {
    KeyCode::Esc => app.close_preview(),
    KeyCode::Char('h') => {
        app.move_left();
        app.sync_preview_to_selection();
    }
    KeyCode::Char('j') => {
        app.move_down();
        app.sync_preview_to_selection();
    }
    KeyCode::Char('k') => {
        app.move_up();
        app.sync_preview_to_selection();
    }
    KeyCode::Char('l') => {
        app.move_right();
        app.sync_preview_to_selection();
    }
    KeyCode::Up => app.preview_scroll_up(),
    KeyCode::Down => app.preview_scroll_down(),
    _ => {}
},
```

Add the `sync_preview_to_selection` method to `App`:

```rust
/// Sync the preview panel to the currently selected card.
/// Called after h/j/k/l navigation in Preview mode.
fn sync_preview_to_selection(&mut self) {
    if let Some(name) = self.selected_session() {
        self.preview_session = Some(name.to_string());
        self.preview_scroll_offset = 0;
        self.refresh_preview();
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ccmux -- preview --no-capture`
Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
git add src/dashboard.rs
git commit -m "feat: add h/j/k/l card navigation and Up/Down scroll in Preview mode"
```

---

### Task 5: Switch to watcher-driven preview refresh

**Files:**
- Modify: `src/dashboard.rs:622-648` (the `tokio::select!` arms in `run_loop`)

- [ ] **Step 1: Update the watcher arm to refresh preview**

In `src/dashboard.rs`, in the `watcher_rx` arm of `tokio::select!`, add a preview refresh call after the transcript is read. Find this block (around line 627-636):

```rust
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
}
```

Replace with:

```rust
if is_transcript {
    if let Some(name) = app.session_for_transcript_path(&event.paths) {
        let changed = app.read_transcript(&name);
        if changed
            && let Some(s) = app.sessions.get(&name)
            && s.status == Status::Idle {
                let name = name.clone();
                app.auto_focus_session(&name);
            }
        // Live-refresh preview if this session is being previewed
        if app.input_mode == InputMode::Preview
            && app.preview_session.as_deref() == Some(&name)
        {
            app.refresh_preview();
        }
    }
}
```

- [ ] **Step 2: Remove the tick-based preview refresh**

In the tick arm of `tokio::select!` (around line 642-648), remove the preview refresh:

```rust
// Old:
_ = tick.tick() => {
    // 1-second tick for age display refresh — triggers redraw
    // Also refresh preview transcript on each tick
    if app.input_mode == InputMode::Preview {
        app.refresh_preview();
    }
}

// New:
_ = tick.tick() => {
    // 1-second tick for age display refresh — triggers redraw
}
```

- [ ] **Step 3: Run all tests to verify nothing is broken**

Run: `cargo test -p ccmux --no-capture`
Expected: all tests pass

- [ ] **Step 4: Run `cargo clippy` to check for warnings**

Run: `cargo clippy -p ccmux 2>&1`
Expected: no warnings

- [ ] **Step 5: Commit**

```bash
git add src/dashboard.rs
git commit -m "feat: switch preview to watcher-driven live updates, remove tick-based refresh"
```

---

### Task 6: Final integration test and cleanup

**Files:**
- Modify: `src/ui/preview.rs` (cleanup any dead code)
- Modify: `src/dashboard.rs` (cleanup any dead code)

- [ ] **Step 1: Check for dead code**

The old `format_entry` in `src/transcript.rs` is still used by `build_preview_lines` — actually no, we replaced that. Check if `format_entry` is used anywhere else:

Run: `grep -rn 'format_entry' src/`

If only used in tests within `transcript.rs`, it can stay. If used nowhere, remove it.

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p ccmux --no-capture`
Expected: all tests pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p ccmux 2>&1`
Expected: no warnings

- [ ] **Step 4: Build release to verify**

Run: `cargo build -p ccmux --release 2>&1`
Expected: successful build

- [ ] **Step 5: Commit any cleanup**

```bash
git add -A
git commit -m "chore: clean up dead code from preview overhaul"
```
