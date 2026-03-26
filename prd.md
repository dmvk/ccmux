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
          │ inotify          │ writes on hook events
          ▼                  ▼
     /tmp/ccmux/
       trading.json     { status, tool, msg, ts, dir }
       infra.json
       hotfix.json
```

### Session lifecycle

```
ccmux new <name>
  → zellij action new-tab --name <name>
  → CCMUX_SESSION=<name> claude
  → hooks write /tmp/ccmux/<name>.json on every event
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

Each session is a single JSON file at `/tmp/ccmux/<name>.json`.

```json
{
  "status": "waiting",
  "tool": null,
  "msg": "Should I increase position size given recent vol?",
  "ts": 1711234567,
  "dir": "~/speedbets/trading"
}
```

**Status values:**

| Value     | Meaning                               | Source hook      |
|-----------|---------------------------------------|------------------|
| `starting` | session initialising                 | ccmux new        |
| `working`  | tool call in progress                | PreToolUse       |
| `waiting`  | awaiting user input                  | Notification     |
| `idle`     | claude returned control to shell     | Stop             |
| `done`     | claude process exited                | ccmux wrapper    |

---

## 6. Claude Code Hooks

Hooks live in `~/.claude/hooks/ccmux/` and are activated per-session via `CCMUX_SESSION`.

### PreToolUse.sh
```bash
#!/bin/bash
name="${CCMUX_SESSION:-default}"
echo "{\"status\":\"working\",\"tool\":\"${CLAUDE_TOOL_NAME:-?}\",\"msg\":\"\",\"ts\":$(date +%s),\"dir\":\"$(pwd)\"}" \
  > "/tmp/ccmux/${name}.json"
```

### Stop.sh
```bash
#!/bin/bash
name="${CCMUX_SESSION:-default}"
echo "{\"status\":\"idle\",\"tool\":null,\"msg\":\"\",\"ts\":$(date +%s),\"dir\":\"$(pwd)\"}" \
  > "/tmp/ccmux/${name}.json"
```

### Notification.sh (waiting state + Ghostty alert)
```bash
#!/bin/bash
name="${CCMUX_SESSION:-default}"
msg="${CLAUDE_MESSAGE:-needs input}"
echo "{\"status\":\"waiting\",\"tool\":null,\"msg\":\"${msg:0:80}\",\"ts\":$(date +%s),\"dir\":\"$(pwd)\"}" \
  > "/tmp/ccmux/${name}.json"

# Fire native macOS notification via Tailscale reverse SSH (fire-and-forget)
ssh your-mac-tailscale \
  "osascript -e 'display notification \"${msg:0:60}\" with title \"ccmux: ${name}\"'" \
  2>/dev/null &
```

---

## 7. `ccmux` CLI

Single binary. Subcommands:

| Command              | Action                                              |
|----------------------|-----------------------------------------------------|
| `ccmux new <name>`   | Open new Zellij tab, start claude with hooks        |
| `ccmux attach <name>`| `zellij action go-to-tab-name <name>`              |
| `ccmux kill <name>`  | Close tab, remove registry file                     |
| `ccmux list`         | Print session table (used by dashboard internally)  |
| `ccmux dashboard`    | Launch Ratatui TUI (run in dashboard tab)           |

---

## 8. Dashboard TUI

### Layout

```
┌─ ccmux ──────────────────────────────── hetzner-arm64 ── 14:23:01 ─┐
│                                                                      │
│  NEEDS INPUT (2)    WORKING (3)       IDLE (1)       DONE (1)       │
│ ─────────────────┬──────────────────┬──────────────┬──────────────  │
│ ? trading    43s │ ● ml-feats   28s │ ○ docs  14m  │ ✓ deploy  2h  │
│   increase pos.. │   OBI rolling.. │              │   deployed..  │
│   ~/speedbets    │   ~/speedbets   │   ~/speedb.. │   ~/speedb..  │
│ ················ │ ················│ ············ │ ············  │
│ ? infra     112s │ ● backtest  5m  │              │               │
│   delete c6g s.. │   XGBoost cv.. │              │               │
│   ~/speedbets    │   ~/speedbets   │              │               │
│ ················ │ ················│              │               │
│                  │ ● hotfix    17s │              │               │
│                  │   WS reconnec.. │              │               │
│                  │   ~/speedbets   │              │               │
├──────────────────┴──────────────────┴──────────────┴───────────────┤
│ session: trading  status: waiting  dir: ~/speedbets/trading         │
│ ↑↓ navigate · Enter attach · Ctrl+y back · n new · x kill · q quit │
└──────────────────────────────────────────────────────────────────────┘
```

### Status icons

| Icon | Status  |
|------|---------|
| `?`  | waiting |
| `●`  | working |
| `○`  | idle    |
| `✓`  | done    |

### Colour scheme (Ratatui `Style`)

| Element          | Colour              |
|------------------|---------------------|
| waiting icon/border | Yellow           |
| working icon     | Blue                |
| idle             | DarkGray            |
| done             | Green               |
| selected row     | bg: DarkBlue        |
| tool name        | Cyan                |
| directory        | DarkGray            |
| message (waiting)| Yellow              |
| message (working)| Gray                |
| age              | DarkGray            |

### Keyboard bindings

| Key          | Action                                 |
|--------------|----------------------------------------|
| `↑` / `k`   | select previous session                |
| `↓` / `j`   | select next session                    |
| `←` / `h`   | move selection to previous column      |
| `→` / `l`   | move selection to next column          |
| `Enter`      | attach to selected session tab         |
| `Ctrl+y`     | go back to dashboard (Zellij binding)  |
| `n`          | prompt for name, open new session      |
| `x`          | kill selected session (confirm prompt) |
| `c`          | clear done sessions from registry      |
| `q` / `Esc`  | quit dashboard                         |

### Navigation model

Selection is per-column. `←→` switches column, `↑↓` moves within column. The `waiting` column always has focus on first render. Sessions within each column are sorted by age ascending (oldest wait first).

---

## 9. Zellij Integration

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

New CC tabs are created dynamically by `ccmux new`.

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

`Ctrl+Y` from any CC tab → dashboard. `Enter` in dashboard → attach to session. Switching cost: one keypress each direction.

---

## 10. Rust Crate Structure

```
ccmux/
├── Cargo.toml
├── src/
│   ├── main.rs          # CLI entry, subcommand dispatch
│   ├── registry.rs      # Session struct, JSON read/write, inotify watch
│   ├── dashboard.rs     # Ratatui app loop, event handling
│   ├── ui/
│   │   ├── kanban.rs    # Column + card widgets
│   │   └── statusbar.rs # Bottom status line
│   └── zellij.rs        # Command wrappers (new-tab, go-to-tab, close-tab)
```

### Key dependencies

```toml
[dependencies]
ratatui        = "0.26"
crossterm      = "0.27"
notify         = "6"          # inotify for registry dir
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
- Session registry (JSON files, hooks)
- `ccmux new / attach / kill / list / dashboard` CLI
- Ratatui kanban with 4 columns
- inotify live refresh (no polling)
- Keyboard navigation and attach
- Zellij tab integration
- Ghostty notification via reverse SSH on `waiting`
- SSH auto-attach via `.bashrc`

### Deferred (v0.2+)
- Kill confirmation prompt
- `n` new-session prompt within TUI
- `c` clear-done sweep
- Per-session log tail in detail pane
- Multiple remote hosts in one dashboard
- Config file (`~/.config/ccmux/config.toml`)

---

## 13. Open Questions

1. **Notification hook trigger** — Claude Code's `Notification` hook fires for all prints, not just input prompts. Need to filter or debounce to avoid noise.
2. **`done` detection** — requires a wrapper around `claude` to detect process exit. Alternative: a short-lived systemd unit or a trap in the shell wrapper.
3. **Zellij tab listing** — `zellij list-sessions` shows sessions not tabs. Tab enumeration may require `zellij action dump-screen` or piping through the plugin API if needed for reconciliation.
4. **Registry on reboot** — `/tmp/ccmux/` is ephemeral. Fine for now; revisit if persistent history is needed.
