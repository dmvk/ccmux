# Preview Panel Overhaul

Redesign the transcript preview panel for better UX: remove the dark background, add rich color styling, enable card navigation while previewing, add scroll support, and switch to watcher-driven live updates.

## 1. Visual Overhaul

### Background
Remove the `DarkGray` background from the entire preview panel. Use `Style::default()` everywhere so the terminal's native background shows through, matching the kanban area above.

### Header
- Left: session name in bold white
- Right: keybinding hints in dim gray: `"^/v scroll  h/j/k/l navigate  Esc close"`
- Separator below: thin `---` line in DarkGray (matches kanban column separators)

### Entry Styling
- **User messages:** `"User "` label in bold yellow, message text in white
- **Assistant messages:** `"Assistant "` label in bold green, message text in regular gray
- **Tool entries:** `"Tool "` label in bold cyan, tool name in cyan, description in dark gray
- **Turn separators:** Blank line before each `User` entry (except the first). This groups assistant text + tool calls as one turn, with visual breathing room between turns.

### Scroll Indicator
When `preview_scroll_offset > 0` (not at bottom), show `"^^ more"` in the bottom-right corner in dim yellow.

## 2. Navigation and Scrolling

### Card Navigation (h/j/k/l)
In Preview mode, h/j/k/l call the same `move_up/down/left/right` methods as Normal mode. After moving:
1. Update `preview_session` to the newly selected card's name
2. Reload the transcript via `refresh_preview()`
3. Reset `preview_scroll_offset` to 0 (auto-tail)

### Transcript Scrolling (Up/Down arrows)
- `preview_scroll_offset: usize` on App tracks lines scrolled up from the bottom
- Up arrow increments offset (scroll toward older content)
- Down arrow decrements offset (scroll toward newer content)
- Clamp to `0..=(total_lines.saturating_sub(visible_height))`

### Auto-tail
- When `preview_scroll_offset == 0`, the view sticks to the bottom. New content appears automatically.
- When scrolled up, new content does not shift the view. The offset is increased by the number of new lines added, keeping the same content visible.

### Esc
Closes preview, clears `preview_session`, `preview_lines`, and `preview_scroll_offset`. Returns to Normal mode.

## 3. Watcher-Driven Live Updates

### Current Behavior
- Transcript file changes come through `watcher_rx`, calling `read_transcript()` to update card state
- Preview refreshes separately on a 1-second tick

### New Behavior
- When a transcript watcher event fires and the changed session matches `preview_session`, call `refresh_preview()` in the same `tokio::select!` arm alongside the card state update
- Remove the tick-based preview refresh (the `if app.input_mode == InputMode::Preview` block in the tick arm)
- Preview updates are now instant: the same event that updates card status also refreshes the preview

### Scroll Stability on Update
In `refresh_preview()`:
- If `preview_scroll_offset == 0`: view stays at bottom (auto-tail)
- If `preview_scroll_offset > 0`: increase offset by the delta of new lines (`new_len - old_len`) so the same content stays visible

## 4. Data Model Changes

### New Fields on `App`
- `preview_scroll_offset: usize` -- lines scrolled up from bottom, 0 = auto-tail

### Changed Fields
`preview_lines: Vec<String>` becomes `preview_lines: Vec<PreviewLine>` where:

```rust
enum PreviewLine {
    User(String),           // message text
    Assistant(String),      // message text
    Tool { name: String, desc: String },
    Separator,
}
```

### Formatting
`refresh_preview()` builds `PreviewLine` values directly from `TranscriptEntry`:
- `TranscriptEntry::User(text)` -> `PreviewLine::User(text)`
- `TranscriptEntry::Assistant(text)` -> `PreviewLine::Assistant(text)`
- `TranscriptEntry::Tool(detail)` -> `PreviewLine::Tool { name, desc }` (split on first space)
- Insert `PreviewLine::Separator` before each `User` entry (except the first)

The `format_entry()` function is no longer needed for preview rendering (may be kept if used elsewhere).

## Files to Modify

| File | Changes |
|------|---------|
| `src/ui/preview.rs` | Complete rewrite of `render_preview` and `entry_style`. Add `PreviewLine` enum. New scroll indicator rendering. |
| `src/dashboard.rs` | Add `preview_scroll_offset` field. Update `open_preview`/`close_preview`. Update `refresh_preview` to build `PreviewLine` values and handle scroll stability. Add h/j/k/l/Up/Down handlers in Preview input mode. Move preview refresh from tick to watcher arm. |
| `src/transcript.rs` | Split tool detail in `format_entry` to expose name + desc separately (or add a new method). |
