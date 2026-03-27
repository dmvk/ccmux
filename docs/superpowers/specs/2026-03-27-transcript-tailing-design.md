# Transcript Tailing + Tokio Migration

Replace most hooks with transcript-based session state, add context token display, migrate to async tokio event loop.

## Motivation

- Claude Code hooks don't provide token/context usage data
- Transcript JSONL files contain everything: status, tool names, token usage
- Reducing hooks from 5 to 2 simplifies setup and reduces coupling to Claude Code internals
- Tokio enables clean async event loop with `select!` over multiple event sources

## Data Model

### Session struct

```rust
pub struct Session {
    pub status: Status,                   // Starting, Working, Idle, Done
    pub tool: Option<String>,             // current tool name (from transcript)
    pub msg: Option<String>,              // last assistant text snippet
    pub ts: u64,                          // last update timestamp
    pub seq: u64,                         // monotonic counter
    pub dir: Option<String>,              // working directory
    pub transcript_path: Option<String>,  // path to .jsonl file
    pub input_tokens: Option<u64>,        // from transcript usage
}
```

### Status enum

Drop `Waiting`. Keep `Starting`, `Working`, `Idle`, `Done`. `Idle` is the internal name; the column header displays as "NEEDS ATTENTION".

### Field notes

- `msg`: With the Notification hook removed, this field is no longer set by hooks. It can be repurposed to hold a truncated snippet of the last assistant text (from transcript), or removed entirely. For now, keep it in the struct but leave it as `None` — it can be wired up later if useful.
- `transcript_path`: Set once on SessionStart, never updated. Carried forward on subsequent registry writes.
- `input_tokens`: Updated from transcript on every `assistant` line that has `usage` data.

### Columns

3 columns, left to right:

| Column | Status | Meaning |
|--------|--------|---------|
| WORKING | Starting, Working | Claude is calling tools |
| NEEDS ATTENTION | Idle | Claude stopped, user's turn |
| DONE | Done | Session ended |

The old `Waiting` / `NEEDS INPUT` column is merged into NEEDS ATTENTION. The 5-second debounce for Notification-to-Waiting transitions is removed entirely.

## Hooks

Reduced from 5 to 2:

| Hook | Status | Purpose |
|------|--------|---------|
| SessionStart | starting | Register session with `transcript_path` + `cwd` |
| SessionEnd | done | Mark session as done |

Removed: PreToolUse, Notification, Stop. All replaced by transcript tailing.

### SessionStart payload parsing

Extract `transcript_path` and `cwd` from the hook stdin JSON:

```json
{
  "session_id": "abc123",
  "transcript_path": "/Users/.../.claude/projects/.../abc123.jsonl",
  "cwd": "/Users/.../project",
  "hook_event_name": "SessionStart"
}
```

### Registry file on disk

```json
{
  "status": "starting",
  "tool": null,
  "msg": null,
  "ts": 1711234567,
  "seq": 0,
  "dir": "~/projects/trade",
  "transcript_path": "/Users/bob/.claude/projects/-Users-bob-Workspace-trade/abc123.jsonl"
}
```

### Backwards compatibility

Old hooks (PreToolUse, Notification, Stop) left in user settings are harmless. `emit.rs` silently ignores unknown status values instead of erroring.

## Transcript Tailing

### Architecture

Single `notify` watcher handles both registry dir and transcript files. Event-driven: no polling, no timers for file reads.

1. Session registers with `transcript_path` via SessionStart hook
2. Dashboard detects new registry file, reads `transcript_path`, adds it to `notify` watcher
3. On transcript file change event: seek to last byte offset, read new bytes, parse lines
4. Extract state from `assistant` type JSONL lines, update session
5. When session is killed/removed, unwatch the transcript file

### State derivation from transcript

Each `assistant` entry in the JSONL has:

```json
{
  "type": "assistant",
  "message": {
    "stop_reason": "tool_use" | "end_turn",
    "content": [{"type": "tool_use", "name": "Bash", ...}, ...],
    "usage": {
      "input_tokens": 3,
      "cache_creation_input_tokens": 7860,
      "cache_read_input_tokens": 12300,
      "output_tokens": 30
    }
  }
}
```

Status derivation:

- `stop_reason: "tool_use"` with tool_use content blocks -> `Working`, extract tool name
- `stop_reason: "end_turn"` -> `Idle` (displayed as NEEDS ATTENTION)
- `stop_reason: null` (streaming chunks) -> ignore for status, grab `usage` if present

Token calculation:

```
input_tokens = usage.input_tokens
             + usage.cache_creation_input_tokens
             + usage.cache_read_input_tokens
```

### Transcript update message

```rust
pub struct TranscriptUpdate {
    pub session_name: String,
    pub status: Status,
    pub tool: Option<String>,
    pub input_tokens: Option<u64>,
    pub ts: u64,
}
```

### Byte offset tracking

Dashboard maintains `HashMap<String, u64>` mapping session names to last-read byte offset. On file change:

1. Open file, seek to stored offset
2. Read to EOF
3. Split into lines, parse JSON for `assistant` type entries
4. Update offset to current file position

## Tokio Migration

### Event loop

Replace synchronous `crossterm::event::poll` + `notify` drain loop with async `tokio::select!`:

```rust
async fn run_loop(terminal, app) {
    let mut key_stream = EventStream::new();

    loop {
        terminal.draw(..)?;

        tokio::select! {
            Some(key) = key_stream.next() => {
                handle_key(key?, app);
            }
            Some(event) = watcher_rx.recv() => {
                app.handle_watcher_event(event);
            }
            _ = tick.tick() => {
                // 1-second tick for age display refresh
            }
        }
    }
}
```

The watcher channel carries events from both registry dir changes and transcript file changes. `handle_watcher_event` dispatches based on the file path.

### Dependency changes

```toml
[dependencies]
tokio = { version = "1", features = ["rt", "macros", "time", "fs", "io-util"] }
futures = "0.3"
crossterm = { version = "0.28", features = ["event-stream"] }
# existing deps unchanged
```

### What changes

- `main.rs`: `fn main()` -> `#[tokio::main] async fn main()`
- `dashboard.rs`: sync event loop -> `tokio::select!` loop
- `dashboard.rs`: `std::sync::mpsc` -> `tokio::sync::mpsc::unbounded_channel`
- `App` struct: add `transcript_offsets: HashMap<String, u64>` for byte tracking
- `emit.rs`: parse `transcript_path` from SessionStart payload, silently ignore unknown statuses
- `init.rs`: reduce `ccmux_hooks()` from 5 to 2 entries
- New `src/transcript.rs`: JSONL parsing and state extraction

### What stays the same

- All rendering code (kanban.rs, statusbar.rs, modal.rs)
- Navigation logic (move_up/down/left/right, clamp_selections)
- Modal input handling
- Registry module (read/write session files, validation)
- Zellij module

## Card Rendering

### Token bar on line 1

```
● trading  ▰▰▰▱▱▱▱▱ 34k  14m
  Edit
  ~/projects/trade
```

Line 1 layout: `icon name [token_bar token_count] age`

- Token bar: 8 blocks, each ~12.5k tokens (100k / 8)
- Filled block: `▰`, empty: `▱`
- Token count: `34k` format (rounded to nearest k)
- Bar + count hidden until first `assistant` line with usage data arrives

### Token bar color thresholds

Scale is 0-100k tokens (practical ceiling, not theoretical model limit).

- Green: 0-40k (< 40%)
- Yellow: 40k-70k (40-70%)
- Red: 70k+ (> 70%)

### Column display

3 columns: WORKING -> NEEDS ATTENTION -> DONE

Auto-focus jumps to NEEDS ATTENTION when a session transitions there (detected from transcript `end_turn`).
