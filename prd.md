# ccmux — Product Requirements Document

**Status:** Draft
**Target:** v0.1.0 MVP
**Stack:** Rust, Ratatui, CrosstermBackend, notify
**Deploy:** Hetzner ARM64 (aarch64-unknown-linux-gnu)

---

## 1. Problem

Running 5+ Claude Code sessions simultaneously over SSH is unmanageable:

- No visibility into which sessions need input without manually switching to each tab
- Sessions waiting for a yes/no block silently — no notification
- No way to see all session states at a glance
- Tab switching in Zellij is manual and context-breaking

---

## 2. Goals

- **Single glance status** — know which sessions need input without attaching to them
- **Keyboard-first** — navigate and act without leaving the dashboard
- **SSH-transparent** — works fully over SSH, Blink Shell, any terminal
- **Zero dependencies on Claude internals** — hooks write files, dashboard reads files
- **Incrementally shippable** — MVP in a weekend, polish later

---

## 3. Non-Goals

- No Zellij plugin (WASM) for v0.1 — separate tab is sufficient
- No web UI or HTTP API
- No cross-machine session aggregation
- No input forwarding to sessions (attach and type manually)
- No macOS/Ghostty notifications (deferred)

---

## 4. Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ ccmux Zellij session (persistent, SSH auto-attach)          │
│                                                             │
│  tab: dashboard   tab: trading   tab: infra   tab: hotfix   │
│  ┌────────────┐   ┌───────────┐  ┌─────────┐  ┌─────────┐  │
│  │ ccmux TUI  │   │  claude   │  │ claude  │  │ claude  │  │
│  │ (Ratatui)  │   │  process  │  │ process │  │ process │  │
│  └────────────┘   └───────────┘  └─────────┘  └─────────┘  │
└─────────────────────────────────────────────────────────────┘
          │                  │
          │ inotify          │ ccmux emit (via hooks)
          ▼                  ▼
     ~/.ccmux/
       trading.json     { status, tool, msg, ts, seq, dir }
       infra.json
       hotfix.json
```

### Data flow

```
Claude Code hook event
  → stdin JSON payload
  → hook command: ccmux emit --status <status> --name "$CCMUX_SESSION"
  → ccmux emit parses stdin, serializes JSON, atomic write (tmp + rename)
  → dashboard detects change via inotify
```

### Session lifecycle

```
ccmux new <name>
  → zellij action new-tab --name <name>
  → CCMUX_SESSION=<name> claude
  → hooks fire ccmux emit on every event
  → dashboard picks up changes via inotify
```

### SSH entry point

```bash
# ~/.bashrc / ~/.zshrc
if [[ -z "$ZELLIJ" && -n "$SSH_CONNECTION" ]]; then
  zellij attach ccmux 2>/dev/null \
    || zellij -s ccmux --layout ~/.config/zellij/layouts/ccmux.kdl
fi
```

---

## 5. Session Registry

Each session is a single JSON file at `~/.ccmux/<name>.json`.

Written exclusively by `ccmux emit`. Writes are atomic (write to tempfile + `rename()`).

```json
{
  "status": "waiting",
  "tool": null,
  "msg": "Should I increase position size given recent vol?",
  "ts": 1711234567,
  "seq": 42,
  "dir": "~/speedbets/trading"
}
```

### Fields

| Field    | Description                                                   |
|----------|---------------------------------------------------------------|
| `status` | One of: `starting`, `working`, `waiting`, `idle`, `done`      |
| `tool`   | Tool name when `working`, null otherwise                      |
| `msg`    | Message text when `waiting`, empty otherwise (truncated 80ch) |
| `ts`     | Unix timestamp of last update                                 |
| `seq`    | Monotonically increasing sequence number (per session)        |
| `dir`    | Project directory, set once at session creation, never updated|

### Status values

| Value      | Meaning                           | Source hook    |
|------------|-----------------------------------|----------------|
| `starting` | Session initialising              | SessionStart   |
| `working`  | Tool call in progress             | PreToolUse     |
| `waiting`  | Awaiting user input               | Notification   |
| `idle`     | Claude returned control to shell  | Stop           |
| `done`     | Claude process exited             | SessionEnd     |

### Session name constraints

- Characters: `[a-zA-Z0-9-]`
- Max length: 20 characters
- Must be unique (reject duplicates)

---

## 6. Claude Code Hooks

Hooks are configured in `~/.claude/settings.json` via `ccmux init`. Each hook calls `ccmux emit` which reads the Claude Code JSON payload from stdin and writes the session registry file.

Sessions are identified by the `CCMUX_SESSION` environment variable, set by `ccmux new`.

### Hook configuration (installed by `ccmux init`)

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "ccmux emit --status starting"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "ccmux emit --status working"
          }
        ]
      }
    ],
    "Notification": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "ccmux emit --status waiting"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "ccmux emit --status idle"
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "ccmux emit --status done"
          }
        ]
      }
    ]
  }
}
```

### `ccmux emit` behavior

- Reads `CCMUX_SESSION` env var for session name (skips silently if unset)
- Reads Claude Code JSON payload from stdin to extract tool name, message, etc.
- Increments the session's sequence number
- Writes JSON atomically (tempfile + `rename()`) to `~/.ccmux/<name>.json`
- The `dir` field is only written on `--status starting`, preserved on subsequent writes

---

## 7. `ccmux` CLI

Single binary. Subcommands:

| Command               | Action                                                    |
|-----------------------|-----------------------------------------------------------|
| `ccmux init`          | Install hooks into `~/.claude/settings.json` (diff + confirm) |
| `ccmux new <name>`    | Validate name, open Zellij tab, `CCMUX_SESSION=<name> claude` |
| `ccmux attach <name>` | `zellij action go-to-tab-name <name>`                     |
| `ccmux kill <name>`   | Close Zellij tab, remove registry file                    |
| `ccmux list`          | Print session table to stdout                             |
| `ccmux emit`          | Write session status (called by hooks, not users)         |
| `ccmux dashboard`     | Launch Ratatui TUI (run in dashboard tab)                 |

---

## 8. Dashboard TUI

### Layout

```
┌─ ccmux ──────────────────────────────── hetzner-arm64 ── 14:23:01 ─┐
│                                                                      │
│  NEEDS INPUT (2)    WORKING (3)       IDLE (1)       DONE (1)       │
│ ─────────────────┬──────────────────┬──────────────┬──────────────  │
│ ? trading    43s │ ● ml-feats   28s │ ○ docs  14m  │ ✓ deploy  2h  │
│   increase pos.. │   Edit          │              │              │
│   ~/speedbets    │   ~/speedbets   │   ~/speedb.. │   ~/speedb..  │
│ ················ │ ················│ ············ │ ············  │
│ ? infra     112s │ ● backtest  5m  │              │               │
│   delete c6g s.. │   Bash          │              │               │
│   ~/speedbets    │   ~/speedbets   │              │               │
│ ················ │ ················│              │               │
│                  │ ● hotfix    17s │              │               │
│                  │   Write         │              │               │
│                  │   ~/speedbets   │              │               │
├──────────────────┴──────────────────┴──────────────┴───────────────┤
│ session: trading  status: waiting  dir: ~/speedbets/trading         │
│ h/j/k/l navigate · Enter attach · Ctrl+y back · x kill · q quit    │
└──────────────────────────────────────────────────────────────────────┘
```

### Kanban behavior

- Empty columns are **hidden** — remaining columns expand to fill the space
- Sessions within each column sorted by age ascending (oldest first)
- On startup, any existing session files are swept (marked `done` or deleted)

### Status icons

| Icon | Status  |
|------|---------|
| `?`  | waiting |
| `●`  | working |
| `○`  | idle    |
| `✓`  | done    |

### Colour scheme (Ratatui `Style`)

| Element             | Colour   |
|---------------------|----------|
| waiting icon/border | Yellow   |
| working icon        | Blue     |
| idle                | DarkGray |
| done                | Green    |
| selected row        | bg: DarkBlue |
| tool name           | Cyan     |
| directory           | DarkGray |
| message (waiting)   | Yellow   |
| message (working)   | Gray     |
| age                 | DarkGray |

### Keyboard bindings (vim-only)

| Key      | Action                                |
|----------|---------------------------------------|
| `k`      | select previous session               |
| `j`      | select next session                   |
| `h`      | move selection to previous column     |
| `l`      | move selection to next column         |
| `Enter`  | attach to selected session tab        |
| `Ctrl+y` | go back to dashboard (Zellij binding) |
| `x`      | kill selected session                 |
| `q`      | quit dashboard                        |

### Navigation model

- Selection is per-column. `h/l` switches column (skipping hidden empties), `j/k` moves within column.
- **Auto-focus**: when a session transitions to `waiting`, selection auto-jumps to it.
- The `waiting` column has focus on first render.

### Debounce

The dashboard holds a 5-second debounce on `Notification` → `waiting` transitions. If a `PreToolUse` event arrives within that window, the session stays visually in `working` and never flashes yellow. This prevents false "needs input" signals from non-prompt notifications.

---

## 9. Zellij Integration

Hard dependency on Zellij. Zellij calls are isolated in `zellij.rs` for future portability.

### Layout (`~/.config/zellij/layouts/ccmux.kdl`)

```kdl
layout {
  tab name="dashboard" focus=true {
    pane command="ccmux" {
      args "dashboard"
    }
  }
}
```

New session tabs are created dynamically by `ccmux new`.

### Return-to-dashboard keybinding (`~/.config/zellij/config.kdl`)

```kdl
keybinds {
  shared {
    bind "Ctrl y" {
      GoToTabName "dashboard"
    }
  }
}
```

`Ctrl+Y` from any session tab → dashboard. `Enter` in dashboard → attach to session.

---

## 10. Rust Crate Structure

```
ccmux/
├── Cargo.toml
├── src/
│   ├── main.rs          # CLI entry, subcommand dispatch (clap)
│   ├── registry.rs      # Session struct, JSON read/write, inotify watch
│   ├── emit.rs          # ccmux emit: stdin parsing, atomic writes
│   ├── dashboard.rs     # Ratatui app loop, event handling, debounce
│   ├── ui/
│   │   ├── kanban.rs    # Column + card widgets
│   │   └── statusbar.rs # Bottom status line
│   └── zellij.rs        # Command wrappers (new-tab, go-to-tab, close-tab)
```

### Event loop

Single-threaded poll loop (no async runtime). `crossterm::event::poll()` with 1-second timeout. On each tick:
1. Drain crossterm keyboard events
2. Drain `notify` file watcher events
3. Update age display
4. Process debounce timers

### Key dependencies

```toml
[dependencies]
ratatui        = "0.26"
crossterm      = "0.27"
notify         = "6"          # inotify / kqueue for registry dir
serde          = { version = "1", features = ["derive"] }
serde_json     = "1"
clap           = { version = "4", features = ["derive"] }
anyhow         = "1"
```

---

## 11. Build & Deploy

```bash
# Build for Hetzner ARM64 (native on the box)
cargo build --release

# Or cross-compile from mac
cross build --release --target aarch64-unknown-linux-gnu

# Install
scp target/aarch64-unknown-linux-gnu/release/ccmux hetzner:~/.local/bin/
```

No Docker needed — pure Rust binary, no runtime deps.

---

## 12. MVP Scope (v0.1)

### In scope

- `ccmux init` — hook installation into `settings.json` with diff + confirm
- `ccmux new / attach / kill / list / emit / dashboard` CLI
- Session registry at `~/.ccmux/` with atomic writes and sequence numbers
- Session name validation (`[a-zA-Z0-9-]`, max 20 chars)
- Directory set once at session creation
- Hooks: `SessionStart`, `PreToolUse`, `Notification`, `Stop`, `SessionEnd`
- Ratatui kanban with hidden empty columns
- 5s debounce on `waiting` transitions
- Auto-focus on newly `waiting` sessions
- Vim-only navigation (`h/j/k/l`)
- Startup sweep of stale session files
- Single-threaded poll loop (no async)
- inotify/kqueue live refresh
- Zellij tab integration
- SSH auto-attach via `.bashrc`

### Deferred (v0.2+)

- macOS/Ghostty notifications (via reverse SSH or other)
- `PostToolUse` visual differentiation (thinking vs. tool-active)
- Kill confirmation prompt
- `n` new-session prompt within TUI
- `c` clear-done sweep within TUI
- Per-session log tail in detail pane
- Multiple remote hosts in one dashboard
- Multiplexer abstraction (tmux support)
- Adaptive layout for narrow terminals
- Config file (`~/.config/ccmux/config.toml`)
