# ccmux

A Claude Code session multiplexer — a terminal dashboard for managing multiple concurrent Claude Code sessions over SSH.

Single-glance visibility into which sessions need input, which are working, and which are idle.

```
┌─ ccmux ─────────────────────────────────────────────────────┐
│ NEEDS INPUT (2)    WORKING (3)       IDLE (1)    DONE (1)   │
│─────────────────┬──────────────────┬───────────┬───────────│
│ ? trading   43s │ ● ml-feats  28s  │ ○ docs 14m│ ✓ deploy  │
│   increase p..  │   Edit           │           │           │
│   ~/speedbets   │   ~/speedbets    │           │           │
│ ················│ ················ │           │           │
│ ? infra    112s │ ● backtest  5m   │           │           │
│   delete c6g .. │   Bash           │           │           │
├─────────────────┴──────────────────┴───────────┴───────────┤
│ h/j/k/l navigate · Enter attach · n new · x kill · q quit  │
└─────────────────────────────────────────────────────────────┘
```

## Requirements

- [Zellij](https://zellij.dev/) terminal multiplexer
- [Claude Code](https://docs.anthropic.com/en/docs/claude-code) CLI
- Rust toolchain (to build)

## Install

```bash
cargo build --release
cp target/release/ccmux ~/.local/bin/
```

## Setup

### 1. Install Claude Code hooks

```bash
ccmux init
```

This merges ccmux hooks into `~/.claude/settings.json` (shows a diff and asks for confirmation).

### 2. Add Zellij keybinding

Add `Ctrl+Y` to return to the dashboard from any session tab. In `~/.config/zellij/config.kdl`:

```kdl
keybinds {
    shared_except "locked" {
        bind "Ctrl y" { GoToTabName "dashboard"; }
    }
}
```

If your config uses `clear-defaults=true`, add the `bind` line inside your existing `shared_except "locked"` block.

### 3. Create the Zellij layout

Create `~/.config/zellij/layouts/ccmux.kdl`:

```kdl
layout {
    tab name="dashboard" focus=true {
        pane command="ccmux" {
            args "dashboard"
        }
    }
}
```

### 4. SSH auto-attach (optional)

Add to `~/.bashrc` or `~/.zshrc` on the remote host:

```bash
if [[ -z "$ZELLIJ" && -n "$SSH_CONNECTION" ]]; then
    zellij attach ccmux 2>/dev/null \
        || zellij -s ccmux --layout ~/.config/zellij/layouts/ccmux.kdl
fi
```

## Usage

### CLI

| Command               | Action                                           |
|-----------------------|--------------------------------------------------|
| `ccmux dashboard`     | Launch the TUI dashboard                         |
| `ccmux new <name>`    | Create a new Claude session in a Zellij tab      |
| `ccmux attach <name>` | Switch to a session's tab                        |
| `ccmux kill <name>`   | Close session tab and remove registry file       |
| `ccmux list`          | Print session table to stdout                    |
| `ccmux init`          | Install hooks into `~/.claude/settings.json`     |

### Dashboard keybindings

| Key       | Action                          |
|-----------|---------------------------------|
| `j` / `k` | Move selection up/down          |
| `h` / `l` | Move between columns            |
| `Enter`   | Attach to selected session      |
| `n`       | Open new session dialog         |
| `x`       | Kill selected session           |
| `Ctrl+Y`  | Return to dashboard (Zellij)    |
| `q`       | Quit dashboard                  |

### Session statuses

| Icon | Status    | Meaning                     |
|------|-----------|-----------------------------|
| `?`  | waiting   | Awaiting user input         |
| `●`  | working   | Tool call in progress       |
| `○`  | idle      | Control returned to shell   |
| `✓`  | done      | Claude process exited       |

## How it works

Each Claude session runs in a separate Zellij tab with `--worktree` for git isolation. Claude Code hooks fire `ccmux emit` on every lifecycle event, writing session state as JSON files to `~/.ccmux/`. The dashboard watches this directory and renders a live kanban board.

Sessions automatically launch with `--dangerously-skip-permissions` and `--worktree`.

## Cross-compile for ARM64

```bash
cross build --release --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/ccmux hetzner:~/.local/bin/
```
