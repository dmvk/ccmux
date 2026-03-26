// Transcript reading: locate and parse Claude Code JSONL transcripts.
//
// Claude Code stores conversation transcripts at:
//   ~/.claude/projects/<encoded-path>/<sessionId>.jsonl
//
// Path encoding: replace every `/` with `-`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

/// A parsed transcript entry for display in the preview panel.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptEntry {
    User(String),
    Assistant(String),
    Tool(String),
}

/// Encode a project directory path into the Claude projects directory name.
/// Replaces every `/` with `-`.
pub fn encode_project_path(dir: &str) -> String {
    dir.replace('/', "-")
}

/// Build the path to a session's transcript JSONL file.
/// Returns `None` if the file doesn't exist.
pub fn transcript_path(dir: &str, session_id: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let encoded = encode_project_path(dir);
    let path = PathBuf::from(home)
        .join(".claude")
        .join("projects")
        .join(&encoded)
        .join(format!("{session_id}.jsonl"));
    if path.exists() {
        Some(path)
    } else {
        None
    }
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
            .map(|s| shorten_path(s))
            .unwrap_or_default(),
        "Edit" | "Write" => input
            .get("file_path")
            .and_then(|f| f.as_str())
            .map(|s| shorten_path(s))
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

/// Format a transcript entry for display in the preview panel.
pub fn format_entry(entry: &TranscriptEntry) -> String {
    match entry {
        TranscriptEntry::User(text) => format!("User: {text}"),
        TranscriptEntry::Assistant(text) => format!("Assistant: {text}"),
        TranscriptEntry::Tool(detail) => format!("Tool: {detail}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_project_path_replaces_slashes() {
        assert_eq!(
            encode_project_path("/Users/bob/Workspace/ccmux"),
            "-Users-bob-Workspace-ccmux"
        );
    }

    #[test]
    fn encode_project_path_no_slashes() {
        assert_eq!(encode_project_path("project"), "project");
    }

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
    fn format_user_entry() {
        let entry = TranscriptEntry::User("hello".into());
        assert_eq!(format_entry(&entry), "User: hello");
    }

    #[test]
    fn format_assistant_entry() {
        let entry = TranscriptEntry::Assistant("thinking...".into());
        assert_eq!(format_entry(&entry), "Assistant: thinking...");
    }

    #[test]
    fn format_tool_entry() {
        let entry = TranscriptEntry::Tool("Edit src/main.rs".into());
        assert_eq!(format_entry(&entry), "Tool: Edit src/main.rs");
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
}
