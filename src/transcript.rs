// Transcript reading: locate and parse Claude Code JSONL transcripts.
//
// Claude Code stores conversation transcripts at:
//   ~/.claude/projects/<encoded-path>/<sessionId>.jsonl
//
// Path encoding: replace every `/` with `-`.
//
// Two consumers:
//   1. Preview panel — read_tail_all() provides entries for the preview renderer.
//   2. Status extraction — parse_new_bytes() derives Status/tool/tokens from
//      assistant messages to update the session registry.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use serde_json::Value;

use crate::registry::Status;

// ── Preview panel types + helpers ────────────────────────────────────

/// A parsed transcript entry for display in the preview panel.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptEntry {
    User(String),
    Assistant(String),
    Tool(String),
}


/// Summarize tool input for compact display.
fn summarize_tool_input(tool_name: &str, input: Option<&serde_json::Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };

    match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|c| c.as_str())
            .map(|s| truncate_str(s, 80))
            .unwrap_or_default(),
        "Read" => input
            .get("file_path")
            .and_then(|f| f.as_str())
            .map(shorten_path)
            .unwrap_or_default(),
        "Edit" | "Write" => input
            .get("file_path")
            .and_then(|f| f.as_str())
            .map(shorten_path)
            .unwrap_or_default(),
        "Grep" => input
            .get("pattern")
            .and_then(|p| p.as_str())
            .map(|s| format!("/{}/", truncate_str(s, 40)))
            .unwrap_or_default(),
        "Glob" => input
            .get("pattern")
            .and_then(|p| p.as_str())
            .map(|s| truncate_str(s, 60))
            .unwrap_or_default(),
        "Agent" => input
            .get("description")
            .and_then(|d| d.as_str())
            .map(|s| truncate_str(s, 60))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Shorten a file path for display — keep just the last 2 components.
fn shorten_path(path: &str) -> String {
    let parts: Vec<&str> = path.rsplit('/').take(2).collect();
    if parts.len() == 2 {
        format!("{}/{}", parts[1], parts[0])
    } else {
        path.to_string()
    }
}

/// Truncate a string to at most `max` characters.
fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        // Collapse newlines to spaces for single-line display
        s.replace('\n', " ")
    } else {
        let truncated: String = s.chars().take(max).collect();
        truncated.replace('\n', " ") + "..."
    }
}

/// Parse a JSONL file and return ALL entries (text + tool_use from assistant messages).
/// Unlike `parse_jsonl_line` which returns only the first, this extracts all.
fn parse_jsonl_line_all(line: &str) -> Vec<TranscriptEntry> {
    let val: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let entry_type = match val.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return Vec::new(),
    };

    match entry_type {
        "user" => {
            if val.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Vec::new();
            }
            let content = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("");
            if content.is_empty() {
                Vec::new()
            } else {
                vec![TranscriptEntry::User(truncate_str(content, 200))]
            }
        }
        "assistant" => {
            let Some(content_arr) = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            else {
                return Vec::new();
            };
            let mut entries = Vec::new();
            for block in content_arr {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match block_type {
                    "text" => {
                        let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        if !text.is_empty() {
                            entries.push(TranscriptEntry::Assistant(truncate_str(text, 200)));
                        }
                    }
                    "tool_use" => {
                        let name =
                            block.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        let input_summary = summarize_tool_input(name, block.get("input"));
                        entries
                            .push(TranscriptEntry::Tool(format!("{name} {input_summary}")));
                    }
                    _ => {}
                }
            }
            entries
        }
        _ => Vec::new(),
    }
}

/// Read the tail of a JSONL transcript, extracting all entries from each message.
/// This version extracts all tool_use + text blocks from assistant messages.
pub fn read_tail_all(path: &std::path::Path, max_entries: usize) -> Vec<TranscriptEntry> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let file_len = match file.seek(SeekFrom::End(0)) {
        Ok(len) => len,
        Err(_) => return Vec::new(),
    };

    const CHUNK_SIZE: u64 = 16384;
    let mut buf = Vec::new();
    let mut offset = file_len;

    loop {
        let read_start = offset.saturating_sub(CHUNK_SIZE);
        let read_len = (offset - read_start) as usize;
        if read_len == 0 {
            break;
        }

        if file.seek(SeekFrom::Start(read_start)).is_err() {
            break;
        }

        let mut chunk = vec![0u8; read_len];
        if file.read_exact(&mut chunk).is_err() {
            break;
        }

        chunk.append(&mut buf);
        buf = chunk;
        offset = read_start;

        let newline_count = buf.iter().filter(|&&b| b == b'\n').count();
        if newline_count > max_entries * 2 || offset == 0 {
            break;
        }
    }

    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();

    let mut entries = Vec::new();
    for line in &lines {
        if line.is_empty() {
            continue;
        }
        entries.extend(parse_jsonl_line_all(line));
    }

    // Keep only the last max_entries
    let trim_start = entries.len().saturating_sub(max_entries);
    entries.drain(..trim_start);
    entries
}

// ── Status extraction (used by transcript watcher) ───────────────────

/// Extracted state from a transcript assistant message.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptUpdate {
    pub status: Status,
    pub tool: Option<String>,
    pub desc: Option<String>,
    pub input_tokens: Option<u64>,
}

/// Parse new bytes from a transcript file, returning the latest TranscriptUpdate if any
/// assistant lines were found. Only the last assistant line's state is returned.
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

        // stop_reason: "tool_use" → Working, "end_turn" → Idle, null → Working
        // (null means streaming/intermediate — Claude is actively generating)
        let stop_reason = msg.get("stop_reason").and_then(|s| s.as_str());

        let status = match stop_reason {
            Some("end_turn") => Status::Idle,
            Some("tool_use") | None => Status::Working,
            _ => continue,
        };

        // Extract tool info from content — for both tool_use stop_reason and
        // intermediate (null) messages that may contain tool_use blocks
        let found_tool = msg.get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| {
                arr.iter()
                    .rev()
                    .find(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
            });
        let tool = found_tool.and_then(|item| item.get("name").and_then(|n| n.as_str())).map(String::from);
        let desc = if found_tool.is_some() {
            found_tool.and_then(|item| {
                let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let summary = summarize_tool_input(name, item.get("input"));
                if summary.is_empty() { None } else { Some(summary) }
            })
        } else {
            // No tool_use block — extract text content as a "thinking" indicator
            msg.get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                                item.get("text").and_then(|t| t.as_str()).filter(|s| !s.is_empty())
                            } else {
                                None
                            }
                        })
                        .next_back()
                })
                .map(|s| truncate_str(s, 60))
        };

        let input_tokens = msg.get("usage").map(|usage| {
            let base = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cache_create = usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cache_read = usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            base + cache_create + cache_read
        });

        last_update = Some(TranscriptUpdate {
            status,
            tool,
            desc,
            input_tokens,
        });
    }

    last_update
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Preview panel tests ──────────────────────────────────────────

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_long() {
        assert_eq!(truncate_str("hello world!", 5), "hello...");
    }

    #[test]
    fn truncate_str_collapses_newlines() {
        assert_eq!(truncate_str("line1\nline2", 20), "line1 line2");
    }

    #[test]
    fn shorten_path_deep() {
        assert_eq!(shorten_path("/a/b/c/d.rs"), "c/d.rs");
    }

    #[test]
    fn shorten_path_shallow() {
        assert_eq!(shorten_path("d.rs"), "d.rs");
    }

    #[test]
    fn parse_user_message() {
        let line = r#"{"type":"user","message":{"role":"user","content":"fix the bug"},"uuid":"abc"}"#;
        let entries = parse_jsonl_line_all(line);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], TranscriptEntry::User("fix the bug".into()));
    }

    #[test]
    fn parse_user_meta_skipped() {
        let line = r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"system stuff"}}"#;
        let entries = parse_jsonl_line_all(line);
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_assistant_with_text_and_tool() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Let me fix that."},{"type":"tool_use","name":"Edit","input":{"file_path":"/src/main.rs"}}]}}"#;
        let entries = parse_jsonl_line_all(line);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            TranscriptEntry::Assistant("Let me fix that.".into())
        );
        assert!(matches!(&entries[1], TranscriptEntry::Tool(s) if s.starts_with("Edit")));
    }

    #[test]
    fn parse_progress_skipped() {
        let line = r#"{"type":"progress","data":{"type":"hook_progress"}}"#;
        let entries = parse_jsonl_line_all(line);
        assert!(entries.is_empty());
    }

    #[test]
    fn summarize_bash_input() {
        let input: serde_json::Value =
            serde_json::from_str(r#"{"command":"cargo test"}"#).unwrap();
        assert_eq!(summarize_tool_input("Bash", Some(&input)), "cargo test");
    }

    #[test]
    fn summarize_edit_input() {
        let input: serde_json::Value =
            serde_json::from_str(r#"{"file_path":"/a/b/c/main.rs"}"#).unwrap();
        assert_eq!(summarize_tool_input("Edit", Some(&input)), "c/main.rs");
    }

    #[test]
    fn summarize_grep_input() {
        let input: serde_json::Value =
            serde_json::from_str(r#"{"pattern":"Session"}"#).unwrap();
        assert_eq!(summarize_tool_input("Grep", Some(&input)), "/Session/");
    }

    #[test]
    fn read_tail_all_nonexistent_file() {
        let entries = read_tail_all(std::path::Path::new("/nonexistent"), 10);
        assert!(entries.is_empty());
    }

    #[test]
    fn read_tail_all_from_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        let content = r#"{"type":"user","message":{"role":"user","content":"hello"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi there"}]}}
{"type":"progress","data":{}}
{"type":"user","message":{"role":"user","content":"do the thing"}}
"#;
        std::fs::write(&path, content).unwrap();

        let entries = read_tail_all(&path, 50);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], TranscriptEntry::User("hello".into()));
        assert_eq!(entries[1], TranscriptEntry::Assistant("hi there".into()));
        assert_eq!(entries[2], TranscriptEntry::User("do the thing".into()));
    }

    #[test]
    fn read_tail_all_respects_max_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        let mut content = String::new();
        for i in 0..20 {
            content.push_str(&format!(
                r#"{{"type":"user","message":{{"role":"user","content":"msg {i}"}}}}"#
            ));
            content.push('\n');
        }
        std::fs::write(&path, content).unwrap();

        let entries = read_tail_all(&path, 5);
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0], TranscriptEntry::User("msg 15".into()));
        assert_eq!(entries[4], TranscriptEntry::User("msg 19".into()));
    }

    // ── Status extraction tests ──────────────────────────────────────

    #[test]
    fn parse_tool_use_assistant_line() {
        let line = r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","id":"x","input":{}}],"usage":{"input_tokens":1000,"cache_creation_input_tokens":5000,"cache_read_input_tokens":2000,"output_tokens":50}}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.status, Status::Working);
        assert_eq!(update.tool.as_deref(), Some("Bash"));
        assert!(update.desc.is_none()); // empty input → no desc
        assert_eq!(update.input_tokens, Some(8000));
    }

    #[test]
    fn parse_tool_use_extracts_desc() {
        let line = r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","id":"x","input":{"command":"cargo test"}}],"usage":{"input_tokens":1000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":50}}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.tool.as_deref(), Some("Bash"));
        assert_eq!(update.desc.as_deref(), Some("cargo test"));
    }

    #[test]
    fn parse_edit_tool_extracts_file_path_desc() {
        let line = r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Edit","id":"x","input":{"file_path":"/a/b/src/main.rs"}}],"usage":{"input_tokens":1000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":50}}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.tool.as_deref(), Some("Edit"));
        assert_eq!(update.desc.as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn parse_end_turn_assistant_line() {
        let line = r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Done."}],"usage":{"input_tokens":2000,"cache_creation_input_tokens":0,"cache_read_input_tokens":10000,"output_tokens":100}}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.status, Status::Idle);
        assert!(update.tool.is_none());
        assert_eq!(update.desc.as_deref(), Some("Done."));
        assert_eq!(update.input_tokens, Some(12000));
    }

    #[test]
    fn parse_streaming_chunk_empty_content() {
        // Empty content array — Claude just started, no text yet
        let line = r#"{"type":"assistant","message":{"stop_reason":null,"content":[],"usage":{"input_tokens":500,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":10}}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.status, Status::Working);
        assert!(update.tool.is_none());
        assert!(update.desc.is_none()); // no text content either
        assert_eq!(update.input_tokens, Some(500));
    }

    #[test]
    fn parse_streaming_chunk_with_text_extracts_it() {
        // Intermediate message with text but no tool — Claude is thinking/writing
        let line = r#"{"type":"assistant","message":{"stop_reason":null,"content":[{"type":"text","text":"Let me look at the code and figure out what's happening."}],"usage":{"input_tokens":600,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":15}}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.status, Status::Working);
        assert!(update.tool.is_none());
        assert_eq!(update.desc.as_deref(), Some("Let me look at the code and figure out what's happening."));
    }

    #[test]
    fn parse_streaming_chunk_with_tool_extracts_it() {
        // Intermediate message with tool_use block but stop_reason: null
        let line = r#"{"type":"assistant","message":{"stop_reason":null,"content":[{"type":"tool_use","name":"Grep","id":"x","input":{"pattern":"Session"}}],"usage":{"input_tokens":800,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":20}}}"#;
        let update = parse_new_bytes(line.as_bytes()).unwrap();
        assert_eq!(update.status, Status::Working);
        assert_eq!(update.tool.as_deref(), Some("Grep"));
        assert_eq!(update.desc.as_deref(), Some("/Session/"));
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
        assert_eq!(update.tool.as_deref(), Some("Edit"));
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
