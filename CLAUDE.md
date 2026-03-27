# ccmux

Claude Code session multiplexer — Rust TUI dashboard for managing concurrent Claude Code sessions over SSH via Zellij.

## Quick Reference

```bash
cargo build                          # build
cargo clippy -- -D warnings          # lint
cargo test                           # 184 tests
cargo fmt                            # format
cargo build 2>&1 && cargo clippy -- -D warnings 2>&1 && cargo test 2>&1  # full check
```

## Architecture

- **CLI dispatch**: `main.rs` uses clap derive macros. Subcommands: `init`, `new`, `attach`, `kill`, `list`, `emit`, `dashboard`
- **File-based registry**: Sessions stored as `~/.ccmux/<name>.json`. Writes are always atomic (tmpfile + rename). `emit.rs` is the sole writer; dashboard is the sole reader
- **Watcher-driven dashboard**: `dashboard.rs` runs a `tokio::select!` loop over `notify` file watcher events (registry dir + transcript files), crossterm key events, and a 1s tick timer. Events bridge sync→async via `mpsc::unbounded_channel`
- **Transcript-derived status**: `transcript.rs` incrementally reads Claude Code JSONL transcripts for real-time tool usage, token counts, and Working/Idle transitions. The `emit` command only handles `starting` and `done` — all other status transitions come from transcript watching
- **Composable UI**: `ui/` splits into `kanban.rs`, `preview.rs`, `modal.rs`, `statusbar.rs`. Rendering uses free functions taking `(&App, Rect, &mut Buffer)` — no Widget trait impls

## Conventions

- **Edition 2024** with let-chains (`if let ... && let ...`). Requires Rust 1.94+
- **Error handling**: `anyhow::Result` everywhere. Use `bail!()` for validation, `.context()` for IO wrapping. No `unwrap()` on user-facing paths
- **Serde**: `#[serde(rename_all = "lowercase")]` on enums, `#[serde(default)]` on new optional fields for backward compat with existing JSON files
- **Testability**: Core functions accept `&Path` for the registry dir (e.g., `emit_to()`, `App::with_registry_dir()`). Tests use `tempfile::tempdir()` for isolation
- **Zellij isolation**: All Zellij interaction lives in `zellij.rs`. No `zellij` string literals in other modules
- **Rendering tests**: `Buffer::empty(Rect::new(0, 0, w, h))` pattern with `buffer_text()` helpers to extract and assert on rendered content

## Workflow

- Read the PRD (`prd.md`) for the canonical specification of all features, statuses, and behaviors
- Start complex tasks in Plan mode. Get approval before implementation
- Break large changes into small, testable chunks
- Run the full check before every commit

## Verification

IMPORTANT: Run before every commit:
```bash
cargo build 2>&1 && cargo clippy -- -D warnings 2>&1 && cargo test 2>&1
```

All three must pass clean. Clippy uses `-D warnings` (warnings are errors).

## Deep Dive (read on demand)

- Product requirements and specification: [prd.md](prd.md)
- Project overview and setup: [README.md](README.md)

## Gotchas

- `CCMUX_SESSION` env var must be set for `emit` to write anything — it exits silently if unset
- `emit` only handles `starting` and `done` statuses (silently ignores others). Working/Idle comes from transcript watching in the dashboard
- Transcript state (`apply_transcript_update`) is ephemeral in-memory only — never written back to registry JSON
- `ccmux new` hardcodes `--dangerously-skip-permissions` and `--worktree` flags for Claude sessions
- Registry listing skips dotfiles (to avoid picking up `.name.json.tmp` atomic write temps)
- The hook binary path is hardcoded as `$HOME/.cargo/bin/ccmux`
