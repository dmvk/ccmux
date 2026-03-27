# Transcript Tailing + Tokio Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace most Claude Code hooks with transcript JSONL tailing, add per-card token usage display, and migrate to an async tokio event loop.

**Architecture:** Dashboard watches transcript files via `notify` to derive session status (working/idle) and token usage, replacing 3 of 5 hooks. Single `notify` watcher handles both registry dir (session discovery) and transcript files (state updates). Async `tokio::select!` loop replaces sync poll/drain loop.

**Tech Stack:** Rust, tokio, ratatui, crossterm (event-stream), notify, serde_json

**Spec:** `docs/superpowers/specs/2026-03-27-transcript-tailing-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `Cargo.toml` | Modify | Add tokio, futures; enable crossterm event-stream |
| `src/main.rs` | Modify | `#[tokio::main]` entry point |
| `src/registry.rs` | Modify | Add `transcript_path` + `input_tokens` to Session; drop `Waiting` status |
| `src/transcript.rs` | Create | JSONL parsing: read new bytes, extract status/tool/tokens from assistant lines |
| `src/emit.rs` | Modify | Parse `transcript_path` from SessionStart; silently ignore unknown statuses |
| `src/init.rs` | Modify | Reduce hooks from 5 to 2 (SessionStart + SessionEnd) |
| `src/dashboard.rs` | Modify | Tokio select loop; manage transcript watches + offsets; drop debounce |
| `src/ui/kanban.rs` | Modify | Token bar rendering on card line 1; 3 columns instead of 4 |
| `src/ui/statusbar.rs` | Modify | Remove `Waiting` references; update status labels |
| `src/ui/modal.rs` | No change | — |

---

### Task 1: Update dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Update Cargo.toml**

```toml
[dependencies]
ratatui = "0.29"
crossterm = { version = "0.28", features = ["event-stream"] }
notify = "6"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
anyhow = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "fs", "io-util"] }
futures = "0.3"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors (new deps unused but available)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add tokio, futures; enable crossterm event-stream"
```

---

### Task 2: Update data model — Session struct + Status enum

**Files:**
- Modify: `src/registry.rs`

- [ ] **Step 1: Write test for new Session fields**

Add to the existing `mod tests` in `src/registry.rs`:

```rust
#[test]
fn session_with_transcript_fields_roundtrip() {
    let session = Session {
        status: Status::Working,
        tool: Some("Edit".into()),
        msg: None,
        ts: 1711234567,
        seq: 42,
        dir: Some("~/project".into()),
        transcript_path: Some("/Users/bob/.claude/projects/foo/abc.jsonl".into()),
        input_tokens: Some(34000),
    };
    let json = serde_json::to_string(&session).unwrap();
    let deserialized: Session = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.transcript_path.as_deref(), Some("/Users/bob/.claude/projects/foo/abc.jsonl"));
    assert_eq!(deserialized.input_tokens, Some(34000));
}

#[test]
fn session_without_new_fields_deserializes() {
    // Old registry files without transcript_path/input_tokens still parse
    let json = r#"{"status":"working","tool":"Bash","msg":null,"ts":100,"seq":1,"dir":"/tmp"}"#;
    let session: Session = serde_json::from_str(json).unwrap();
    assert_eq!(session.status, Status::Working);
    assert!(session.transcript_path.is_none());
    assert!(session.input_tokens.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib registry::tests::session_with_transcript_fields_roundtrip`
Expected: FAIL — `Session` struct doesn't have `transcript_path` or `input_tokens` fields

- [ ] **Step 3: Update Session struct and Status enum**

In `src/registry.rs`, update the `Status` enum — remove `Waiting`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Starting,
    Working,
    #[serde(alias = "waiting")]
    Idle,
    Done,
}
```

The `#[serde(alias = "waiting")]` ensures old registry files with `"status": "waiting"` deserialize as `Idle`.

Update the `Session` struct — add new fields with `#[serde(default)]`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub status: Status,
    pub tool: Option<String>,
    pub msg: Option<String>,
    pub ts: u64,
    pub seq: u64,
    pub dir: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
}
```

- [ ] **Step 4: Fix all compilation errors from Waiting removal**

Search for `Status::Waiting` across the codebase and update each occurrence:

In `src/dashboard.rs`:
- `Column::from_status`: remove `Status::Waiting` arm — `Idle` now maps to the column that was `Waiting`
- `Column` enum: rename `Waiting` to `NeedsAttention`, update `title()` to return `"NEEDS ATTENTION"`
- Remove `DEBOUNCE_DURATION` constant
- Remove `debounce_timers` field from `App`
- Remove `process_debounce_timers()` method
- Remove `effective_column()` debounce logic — just use `Column::from_status` directly
- Remove all debounce-related code in `process_watcher_events()`
- Update `auto_focus_session` to use `Column::NeedsAttention`
- Update `focus_initial_column` to prefer `Column::NeedsAttention`
- Update `COLUMN_ORDER` to 3 columns: `[Column::NeedsAttention, Column::Working, Column::Done]`
  - Note: spec says Working first, but NeedsAttention should be leftmost since it's highest priority

In `src/ui/kanban.rs`:
- Update `header_style` match arms — replace `Column::Waiting` with `Column::NeedsAttention`

In `src/ui/statusbar.rs`:
- Update `status_label` — remove `Waiting` arm, `Idle` maps to whatever label is appropriate

In `src/emit.rs`:
- Remove `"waiting"` from `parse_status()` — it's no longer a valid emit status
- Update tests that use `Status::Waiting` to use `Status::Idle`

In test helpers (dashboard, kanban, emit tests):
- Replace `make_session(Status::Waiting, ..)` with `make_session(Status::Idle, ..)`
- Update column assertions from `Column::Waiting` to `Column::NeedsAttention`
- Remove debounce tests entirely

- [ ] **Step 5: Run all tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/registry.rs src/dashboard.rs src/emit.rs src/ui/kanban.rs src/ui/statusbar.rs
git commit -m "refactor: drop Waiting status, add transcript_path + input_tokens to Session"
```

---

### Task 3: Transcript JSONL parser

**Files:**
- Create: `src/transcript.rs`
- Modify: `src/main.rs` (add `mod transcript`)

- [ ] **Step 1: Write tests for transcript parsing**

Create `src/transcript.rs` with tests:

```rust
use crate::registry::Status;

/// Extracted state from a transcript assistant message.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptUpdate {
    pub status: Status,
    pub tool: Option<String>,
    pub input_tokens: Option<u64>,
}

/// Parse new bytes from a transcript file, returning the latest TranscriptUpdate if any
/// assistant lines were found. Only the last assistant line's state is returned.
pub fn parse_new_bytes(bytes: &[u8]) -> Option<TranscriptUpdate> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tool_use_assistant_line() {
        let line = r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","id":"x","input":{}}],"usage":{"input_tokens":1000,"cache_creation_input_tokens":5000,"cache_read_input_tokens":2000,"output_tokens":50}}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.status, Status::Working);
        assert_eq!(update.tool.as_deref(), Some("Bash"));
        assert_eq!(update.input_tokens, Some(8000));
    }

    #[test]
    fn parse_end_turn_assistant_line() {
        let line = r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Done."}],"usage":{"input_tokens":2000,"cache_creation_input_tokens":0,"cache_read_input_tokens":10000,"output_tokens":100}}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.status, Status::Idle);
        assert!(update.tool.is_none());
        assert_eq!(update.input_tokens, Some(12000));
    }

    #[test]
    fn parse_streaming_chunk_ignored_for_status() {
        // stop_reason is null during streaming — usage may still be present
        let line = r#"{"type":"assistant","message":{"stop_reason":null,"content":[],"usage":{"input_tokens":500,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":10}}}"#;
        let update = parse_new_bytes(line.as_bytes());
        // Null stop_reason lines are ignored (no status change)
        assert!(update.is_none());
    }

    #[test]
    fn parse_non_assistant_lines_ignored() {
        let lines = b"{\"type\":\"user\",\"message\":{}}\n{\"type\":\"progress\",\"data\":{}}\n";
        let update = parse_new_bytes(lines);
        assert!(update.is_none());
    }

    #[test]
    fn parse_multiple_lines_returns_last_assistant() {
        let lines = format!(
            "{}\n{}\n",
            r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Read","id":"x","input":{}}],"usage":{"input_tokens":1000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":10}}}"#,
            r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Edit","id":"y","input":{}}],"usage":{"input_tokens":2000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":20}}}"#,
        );
        let update = parse_new_bytes(lines.as_bytes()).unwrap();
        assert_eq!(update.tool.as_deref(), Some("Edit")); // last one wins
        assert_eq!(update.input_tokens, Some(2000));
    }

    #[test]
    fn parse_missing_usage_returns_none_tokens() {
        let line = r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"hi"}]}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.status, Status::Idle);
        assert!(update.input_tokens.is_none());
    }

    #[test]
    fn parse_multiple_tool_uses_picks_last() {
        let line = r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"text","text":"Let me check"},{"type":"tool_use","name":"Read","id":"a","input":{}},{"type":"tool_use","name":"Bash","id":"b","input":{}}],"usage":{"input_tokens":3000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":30}}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.tool.as_deref(), Some("Bash"));
    }

    #[test]
    fn parse_empty_bytes() {
        assert!(parse_new_bytes(b"").is_none());
    }

    #[test]
    fn parse_malformed_json_skipped() {
        let lines = b"not json\n{\"type\":\"assistant\",\"message\":{\"stop_reason\":\"end_turn\",\"content\":[],\"usage\":{\"input_tokens\":100,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":5}}}\n";
        let update = parse_new_bytes(lines).unwrap();
        assert_eq!(update.status, Status::Idle);
        assert_eq!(update.input_tokens, Some(100));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib transcript::tests`
Expected: FAIL — `parse_new_bytes` has `todo!()`

- [ ] **Step 3: Implement parse_new_bytes**

Replace the `todo!()` in `src/transcript.rs`:

```rust
use serde_json::Value;

use crate::registry::Status;

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptUpdate {
    pub status: Status,
    pub tool: Option<String>,
    pub input_tokens: Option<u64>,
}

/// Parse new bytes from a transcript file, returning the latest TranscriptUpdate if any
/// assistant lines were found with a definitive stop_reason. Only the last such line's
/// state is returned.
pub fn parse_new_bytes(bytes: &[u8]) -> Option<TranscriptUpdate> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut last_update: Option<TranscriptUpdate> = None;

    for line in text.lines() {
        let val: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if val.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }

        let msg = match val.get("message") {
            Some(m) => m,
            None => continue,
        };

        let stop_reason = match msg.get("stop_reason").and_then(|s| s.as_str()) {
            Some(s) => s,
            None => continue, // null stop_reason = streaming chunk, skip
        };

        let status = match stop_reason {
            "tool_use" => Status::Working,
            "end_turn" => Status::Idle,
            _ => continue,
        };

        let tool = if status == Status::Working {
            msg.get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .rev()
                        .find(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                        .and_then(|item| item.get("name").and_then(|n| n.as_str()))
                        .map(String::from)
                })
        } else {
            None
        };

        let input_tokens = msg.get("usage").map(|usage| {
            let base = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cache_create = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cache_read = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            base + cache_create + cache_read
        });

        last_update = Some(TranscriptUpdate {
            status,
            tool,
            input_tokens,
        });
    }

    last_update
}
```

- [ ] **Step 4: Add module declaration**

In `src/main.rs`, add:

```rust
mod transcript;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib transcript::tests`
Expected: all 8 tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/transcript.rs src/main.rs
git commit -m "feat: add transcript JSONL parser for status, tool, and token extraction"
```

---

### Task 4: Update emit to parse transcript_path and reduce to 2 statuses

**Files:**
- Modify: `src/emit.rs`

- [ ] **Step 1: Write test for transcript_path parsing**

Add to `mod tests` in `src/emit.rs`:

```rust
#[test]
fn emit_starting_parses_transcript_path() {
    let dir = tempfile::tempdir().unwrap();
    let payload = r#"{"cwd":"/tmp","transcript_path":"/Users/bob/.claude/projects/foo/abc.jsonl"}"#;
    emit_to(dir.path(), "s1", "starting", payload).unwrap();

    let session = registry::read_session_from(dir.path(), "s1").unwrap().unwrap();
    assert_eq!(session.transcript_path.as_deref(), Some("/Users/bob/.claude/projects/foo/abc.jsonl"));
}

#[test]
fn emit_preserves_transcript_path_across_transitions() {
    let dir = tempfile::tempdir().unwrap();
    let payload = r#"{"cwd":"/tmp","transcript_path":"/path/to/transcript.jsonl"}"#;
    emit_to(dir.path(), "s1", "starting", payload).unwrap();
    emit_to(dir.path(), "s1", "done", "{}").unwrap();

    let session = registry::read_session_from(dir.path(), "s1").unwrap().unwrap();
    assert_eq!(session.transcript_path.as_deref(), Some("/path/to/transcript.jsonl"));
}

#[test]
fn emit_unknown_status_ignored_silently() {
    let dir = tempfile::tempdir().unwrap();
    // Old hooks may still fire with "working", "waiting", "idle" — should not error
    let result = emit_to(dir.path(), "s1", "working", "{}");
    assert!(result.is_ok());
    // No file written since session doesn't exist yet
    let session = registry::read_session_from(dir.path(), "s1").unwrap();
    assert!(session.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib emit::tests::emit_starting_parses_transcript_path`
Expected: FAIL — `HookPayload` doesn't have `transcript_path`

- [ ] **Step 3: Update emit.rs**

Update `HookPayload` to include `transcript_path`:

```rust
struct HookPayload {
    tool_name: Option<String>,
    message: Option<String>,
    cwd: Option<String>,
    transcript_path: Option<String>,
}
```

Update `parse_stdin_payload` to extract `transcript_path`:

```rust
fn parse_stdin_payload(stdin_data: &str) -> HookPayload {
    let val: serde_json::Value = serde_json::from_str(stdin_data).unwrap_or_default();

    let tool_name = val.get("tool_name").and_then(|v| v.as_str()).map(String::from);
    let message = val.get("message").and_then(|v| v.as_str()).map(|s| truncate(s, 80).to_string());
    let cwd = val.get("cwd").and_then(|v| v.as_str()).map(String::from);
    let transcript_path = val.get("transcript_path").and_then(|v| v.as_str()).map(String::from);

    HookPayload { tool_name, message, cwd, transcript_path }
}
```

Update `parse_status` to return `Option<Status>` instead of `Result<Status>`, returning `None` for unknown values:

```rust
fn parse_status(s: &str) -> Option<Status> {
    match s {
        "starting" => Some(Status::Starting),
        "done" => Some(Status::Done),
        _ => None, // unknown statuses silently ignored
    }
}
```

Update `emit_to` to handle `Option<Status>` — return early with `Ok(())` if status is unknown:

```rust
pub fn emit_to(
    registry_dir: &std::path::Path,
    session_name: &str,
    status_str: &str,
    stdin_data: &str,
) -> Result<()> {
    let status = match parse_status(status_str) {
        Some(s) => s,
        None => return Ok(()), // silently ignore unknown statuses
    };
    let payload = parse_stdin_payload(stdin_data);

    let existing = registry::read_session_from(registry_dir, session_name)?;

    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let seq = existing.as_ref().map_or(0, |s| s.seq + 1);

    let dir = if status == Status::Starting {
        payload.cwd.or_else(|| existing.as_ref().and_then(|s| s.dir.clone()))
    } else {
        existing.as_ref().and_then(|s| s.dir.clone())
    };

    let transcript_path = if status == Status::Starting {
        payload.transcript_path.or_else(|| existing.as_ref().and_then(|s| s.transcript_path.clone()))
    } else {
        existing.as_ref().and_then(|s| s.transcript_path.clone())
    };

    let session = Session {
        status,
        tool: None,
        msg: None,
        ts,
        seq,
        dir,
        transcript_path,
        input_tokens: existing.as_ref().and_then(|s| s.input_tokens),
    };

    registry::write_session_to(registry_dir, session_name, &session)?;
    Ok(())
}
```

- [ ] **Step 4: Fix existing emit tests**

Update tests that use old statuses ("working", "waiting", "idle"):
- Tests that expected `Status::Working` from `emit_to(.., "working", ..)` should now expect the emit to be silently ignored (no file written or file unchanged)
- Remove or rewrite tests for `parse_status` to cover only "starting" and "done"
- Update the `emitted_session_appears_in_dashboard` and lifecycle tests to reflect new behavior

- [ ] **Step 5: Run all tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/emit.rs
git commit -m "refactor: emit only handles starting/done, parses transcript_path, ignores unknown statuses"
```

---

### Task 5: Reduce hooks from 5 to 2

**Files:**
- Modify: `src/init.rs`

- [ ] **Step 1: Update hook list**

In `src/init.rs`, change `ccmux_hooks()`:

```rust
fn ccmux_hooks() -> Vec<(&'static str, &'static str)> {
    vec![
        ("SessionStart", "\"$HOME/.cargo/bin/ccmux\" emit --status starting"),
        ("SessionEnd", "\"$HOME/.cargo/bin/ccmux\" emit --status done"),
    ]
}
```

- [ ] **Step 2: Update init test**

Update `merge_into_empty_settings` test:

```rust
#[test]
fn merge_into_empty_settings() {
    let mut settings = json!({});
    let changed = merge_hooks(&mut settings);
    assert!(changed);

    let hooks = settings["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), 2);
    assert!(hooks.contains_key("SessionStart"));
    assert!(hooks.contains_key("SessionEnd"));
}
```

- [ ] **Step 3: Run all tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add src/init.rs
git commit -m "refactor: reduce hooks to SessionStart + SessionEnd only"
```

---

### Task 6: Token bar rendering

**Files:**
- Modify: `src/ui/kanban.rs`
- Modify: `src/dashboard.rs` (add token_bar_style helper)

- [ ] **Step 1: Write tests for token bar rendering helpers**

Add to `src/ui/kanban.rs` tests:

```rust
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
    assert_eq!(token_bar(150000), "▰▰▰▰▰▰▰▰"); // capped at 8
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib ui::kanban::tests::format_tokens_display`
Expected: FAIL — functions don't exist

- [ ] **Step 3: Implement token bar helpers**

Add to `src/ui/kanban.rs`:

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib ui::kanban::tests`
Expected: all kanban tests pass

- [ ] **Step 5: Update card line 1 rendering to include token bar**

In `render_column()`, replace the Line 1 block (the one with icon, name, age) with:

```rust
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
```

- [ ] **Step 6: Write rendering test for token bar on card**

Add to `src/ui/kanban.rs` tests:

```rust
#[test]
fn render_card_with_token_data() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = make_session(Status::Working, 950);
    session.tool = Some("Edit".to_string());
    session.input_tokens = Some(34000);
    write_session_to(dir.path(), "trading", &session).unwrap();

    let app = App::with_registry_dir(dir.path()).unwrap();
    let area = Rect::new(0, 0, 60, 10);
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
```

- [ ] **Step 7: Run all tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 8: Commit**

```bash
git add src/ui/kanban.rs src/dashboard.rs
git commit -m "feat: render token usage bar on card line 1 with green/yellow/red thresholds"
```

---

### Task 7: Tokio migration — async event loop

**Files:**
- Modify: `src/main.rs`
- Modify: `src/dashboard.rs`

- [ ] **Step 1: Make main async**

In `src/main.rs`, change:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => init::run(),
        Commands::New { ref name } => {
            registry::validate_session_name(name)?;
            if registry::read_session(name)?.is_some() {
                anyhow::bail!("session '{name}' already exists");
            }
            let env_var = format!("CCMUX_SESSION={name}");
            zellij::new_tab(name, "env", &[&env_var, "claude", "--dangerously-skip-permissions", "--worktree"], None)?;
            Ok(())
        }
        Commands::Attach { ref name } => zellij::go_to_tab(name),
        Commands::Kill { ref name } => {
            let _ = zellij::close_tab(name);
            registry::remove_session(name)?;
            Ok(())
        }
        Commands::List => {
            // ... unchanged list logic ...
            Ok(())
        }
        Commands::Emit { ref status } => emit::run(status),
        Commands::Dashboard => dashboard::run().await,
    }
}
```

Only `dashboard::run()` is async — all other commands remain synchronous.

- [ ] **Step 2: Convert dashboard event loop to tokio**

In `src/dashboard.rs`, update imports:

```rust
use crossterm::event::EventStream;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::time;
```

Change `App` struct — replace `std::sync::mpsc` fields:

```rust
pub struct App {
    pub sessions: HashMap<String, Session>,
    pub selected_column: usize,
    pub selected_rows: HashMap<Column, usize>,
    watcher_rx: mpsc::UnboundedReceiver<notify::Result<notify::Event>>,
    _watcher: RecommendedWatcher,
    registry_dir: PathBuf,
    pub should_quit: bool,
    pub input_mode: InputMode,
    pub modal_name: String,
    pub modal_dir: String,
    pub modal_field: usize,
    pub modal_error: Option<String>,
    pub default_cwd: String,
    pub pending_focus: Option<String>,
    /// Byte offsets into transcript files for incremental reads.
    pub transcript_offsets: HashMap<String, u64>,
}
```

Update `App::with_registry_dir` to use tokio channel:

```rust
pub fn with_registry_dir(dir: &Path) -> Result<Self> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create registry dir: {}", dir.display()))?;

    let (tx, rx) = mpsc::unbounded_channel();
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
        transcript_offsets: HashMap::new(),
    };

    app.focus_initial_column();
    Ok(app)
}
```

Make `run` and `run_loop` async:

```rust
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

        tokio::select! {
            Some(event) = key_stream.next() => {
                if let Event::Key(key) = event? {
                    if key.kind == KeyEventKind::Press {
                        handle_key(app, key.code);
                    }
                }
            }
            Some(event) = app.watcher_rx.recv() => {
                if event.is_ok() {
                    app.process_watcher_events();
                }
            }
            _ = tick.tick() => {
                // age refresh — triggers redraw on next loop iteration
            }
        }
    }

    Ok(())
}
```

Extract key handling into a helper (same logic, just extracted):

```rust
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
            _ => {}
        },
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
```

Update `process_watcher_events` — simplify by removing debounce logic:

```rust
pub fn process_watcher_events(&mut self) {
    // Drain all pending watcher events
    while self.watcher_rx.try_recv().is_ok() {}

    let old_sessions = std::mem::take(&mut self.sessions);
    self.sessions = load_sessions_from(&self.registry_dir);

    self.clamp_selections();

    // Watch transcripts for new sessions that have a transcript_path
    for (name, session) in &self.sessions {
        if !old_sessions.contains_key(name) {
            if let Some(ref path) = session.transcript_path {
                let path = std::path::Path::new(path);
                if path.exists() {
                    let _ = self._watcher.watch(path, RecursiveMode::NonRecursive);
                }
            }
        }
    }

    // Unwatch transcripts for removed sessions
    for (name, session) in &old_sessions {
        if !self.sessions.contains_key(name) {
            if let Some(ref path) = session.transcript_path {
                let _ = self._watcher.unwatch(std::path::Path::new(path));
                self.transcript_offsets.remove(name);
            }
        }
    }

    // Handle pending focus
    if let Some(ref focus_name) = self.pending_focus
        && self.sessions.contains_key(focus_name)
    {
        let focus_name = focus_name.clone();
        self.pending_focus = None;
        let col = self.sessions.get(&focus_name)
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
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check`
Expected: compiles with warnings about unused imports (old debounce, old mpsc)

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: all tests pass. Some dashboard tests may need `#[tokio::test]` if they call async methods — but most test `App` state directly (no async).

- [ ] **Step 5: Commit**

```bash
git add src/main.rs src/dashboard.rs
git commit -m "feat: migrate event loop to tokio with async select over keys, watcher, and tick"
```

---

### Task 8: Wire transcript tailing into the dashboard

**Files:**
- Modify: `src/dashboard.rs`

This task connects the `notify` watcher events for transcript files to the parser from Task 3, updating session state in real-time.

- [ ] **Step 1: Write test for transcript-driven session update**

Add to `src/dashboard.rs` tests:

```rust
#[test]
fn transcript_update_changes_session_status_and_tokens() {
    let dir = tempfile::tempdir().unwrap();

    // Create a session with a transcript_path
    let transcript_dir = tempfile::tempdir().unwrap();
    let transcript_path = transcript_dir.path().join("session.jsonl");
    std::fs::write(&transcript_path, "").unwrap();

    let session = Session {
        status: Status::Starting,
        tool: None,
        msg: None,
        ts: 100,
        seq: 0,
        dir: Some("/project".into()),
        transcript_path: Some(transcript_path.to_string_lossy().to_string()),
        input_tokens: None,
    };
    write_session_to(dir.path(), "sess", &session).unwrap();

    let mut app = App::with_registry_dir(dir.path()).unwrap();

    // Simulate applying a transcript update
    let update = crate::transcript::TranscriptUpdate {
        status: Status::Working,
        tool: Some("Bash".into()),
        input_tokens: Some(34000),
    };
    app.apply_transcript_update("sess", update);

    let s = &app.sessions["sess"];
    assert_eq!(s.status, Status::Working);
    assert_eq!(s.tool.as_deref(), Some("Bash"));
    assert_eq!(s.input_tokens, Some(34000));
}

#[test]
fn transcript_update_ignored_for_done_session() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session {
        status: Status::Done,
        tool: None,
        msg: None,
        ts: 100,
        seq: 0,
        dir: None,
        transcript_path: None,
        input_tokens: None,
    };
    write_session_to(dir.path(), "sess", &session).unwrap();

    let mut app = App::with_registry_dir(dir.path()).unwrap();
    let update = crate::transcript::TranscriptUpdate {
        status: Status::Working,
        tool: Some("Edit".into()),
        input_tokens: Some(5000),
    };
    app.apply_transcript_update("sess", update);

    // Done sessions should not be overridden by transcript
    assert_eq!(app.sessions["sess"].status, Status::Done);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib dashboard::tests::transcript_update_changes_session_status_and_tokens`
Expected: FAIL — `apply_transcript_update` doesn't exist

- [ ] **Step 3: Implement apply_transcript_update and transcript read**

Add to `App` impl in `src/dashboard.rs`:

```rust
/// Apply a transcript update to a session's in-memory state.
/// Does not modify the registry file — transcript state is ephemeral.
/// Ignores updates for Done sessions (SessionEnd hook is authoritative).
pub fn apply_transcript_update(&mut self, name: &str, update: crate::transcript::TranscriptUpdate) {
    if let Some(session) = self.sessions.get_mut(name) {
        if session.status == Status::Done {
            return; // SessionEnd hook is authoritative
        }
        session.status = update.status;
        session.tool = update.tool;
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
```

- [ ] **Step 4: Integrate transcript reads into watcher event handling**

Update `handle_watcher_event` dispatch in the `select!` loop. The watcher sends events for both registry dir changes and transcript file changes. Distinguish by checking the event path:

In `run_loop`, update the watcher branch:

```rust
Some(event) = app.watcher_rx.recv() => {
    if let Ok(event) = event {
        let is_transcript = event.paths.iter().any(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("jsonl")
        });
        if is_transcript {
            // Find which session owns this transcript and read new data
            let session_name = app.session_for_transcript_path(&event.paths);
            if let Some(name) = session_name {
                let changed = app.read_transcript(&name);
                if changed {
                    // Auto-focus if session just became NeedsAttention
                    if let Some(s) = app.sessions.get(&name) {
                        if s.status == Status::Idle {
                            app.auto_focus_session(&name);
                        }
                    }
                }
            }
        } else {
            app.process_watcher_events();
        }
    }
}
```

Add helper to find session by transcript path:

```rust
/// Find which session name corresponds to a transcript file path.
fn session_for_transcript_path(&self, paths: &[PathBuf]) -> Option<String> {
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
```

- [ ] **Step 5: Run all tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/dashboard.rs
git commit -m "feat: wire transcript tailing into dashboard — status, tool, and tokens update from JSONL"
```

---

### Task 9: Update statusbar for new column/status scheme

**Files:**
- Modify: `src/ui/statusbar.rs`

- [ ] **Step 1: Update status labels and remove Waiting references**

In `src/ui/statusbar.rs`, update `status_label`:

```rust
fn status_label(status: &Status) -> &'static str {
    match status {
        Status::Starting => "starting",
        Status::Working => "working",
        Status::Idle => "needs attention",
        Status::Done => "done",
    }
}
```

Add token count to the status bar line 1 when available. After the `dir` display:

```rust
// After dir, show token count if available
if let Some(tokens) = session.input_tokens {
    let token_str = format!("  tokens: {}k", tokens / 1000);
    buf.set_string(x, area.y, &token_str, Style::default().fg(Color::DarkGray));
    x += token_str.len() as u16;
}
```

- [ ] **Step 2: Run all tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 3: Commit**

```bash
git add src/ui/statusbar.rs
git commit -m "feat: update statusbar with needs-attention label and token count"
```

---

### Task 10: Clean up dead code and run full test suite

**Files:**
- Modify: various (remove `#![allow(dead_code)]` where no longer needed, clean up unused imports)

- [ ] **Step 1: Remove dead code annotations and unused imports**

Run: `cargo clippy -- -W dead_code`

Fix any warnings:
- Remove `#![allow(dead_code)]` from `dashboard.rs` and `kanban.rs` if all code is now reachable
- Remove unused `use` statements for old debounce types, old mpsc, etc.
- Remove any remaining references to `Waiting` status or `DEBOUNCE_DURATION`

- [ ] **Step 2: Run full test suite**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy`
Expected: no warnings

- [ ] **Step 4: Manual smoke test**

Run: `cargo build && ./target/debug/ccmux dashboard`
Expected: dashboard renders with 3 columns (WORKING, NEEDS ATTENTION, DONE)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: clean up dead code, unused imports, and allow(dead_code) annotations"
```
