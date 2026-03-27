// ccmux emit: stdin parsing, atomic writes to session registry
//
// Called by Claude Code hooks. Reads CCMUX_SESSION env var for the session name,
// parses the hook JSON payload from stdin, and atomically writes the session file.

use anyhow::Result;
use std::io::{IsTerminal, Read};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::registry::{self, Session, Status};

/// Parse a --status flag value into a Status enum.
/// Returns None for unknown values (old hooks may still fire with legacy statuses).
fn parse_status(s: &str) -> Option<Status> {
    match s {
        "starting" => Some(Status::Starting),
        "done" => Some(Status::Done),
        _ => None,
    }
}

/// Extract fields from the Claude Code hook stdin JSON payload.
struct HookPayload {
    cwd: Option<String>,
    session_id: Option<String>,
    transcript_path: Option<String>,
}

fn parse_stdin_payload(stdin_data: &str) -> HookPayload {
    let val: serde_json::Value = serde_json::from_str(stdin_data).unwrap_or_default();

    let cwd = val
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(String::from);

    let session_id = val
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(String::from);

    let transcript_path = val
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .map(String::from);

    HookPayload { cwd, session_id, transcript_path }
}

/// Run the emit subcommand.
///
/// Reads CCMUX_SESSION from env, parses stdin, and atomically writes the session file.
/// If CCMUX_SESSION is unset, exits silently with success.
pub fn run(status_str: &str) -> Result<()> {
    // Read session name from env — skip silently if unset
    let session_name = match std::env::var("CCMUX_SESSION") {
        Ok(name) if !name.is_empty() => name,
        _ => return Ok(()),
    };

    // Read stdin (hook JSON payload) — skip when stdin is a terminal (interactive use)
    let mut stdin_data = String::new();
    if !std::io::stdin().is_terminal() {
        std::io::stdin().read_to_string(&mut stdin_data)?;
    }

    let dir = registry::registry_dir()?;
    emit_to(&dir, &session_name, status_str, &stdin_data)
}

/// Core emit logic: parse status + payload, build session, write to registry dir.
/// Factored out of `run` so integration tests can call it directly.
pub fn emit_to(
    registry_dir: &std::path::Path,
    session_name: &str,
    status_str: &str,
    stdin_data: &str,
) -> Result<()> {
    let status = match parse_status(status_str) {
        Some(s) => s,
        None => return Ok(()),
    };
    let payload = parse_stdin_payload(stdin_data);

    // Read existing session to carry forward seq and dir
    let existing = registry::read_session_from(registry_dir, session_name)?;

    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let seq = existing.as_ref().map_or(0, |s| s.seq + 1);

    // dir: set on --status starting, preserved on subsequent writes
    let dir = if status == Status::Starting {
        payload.cwd.or_else(|| existing.as_ref().and_then(|s| s.dir.clone()))
    } else {
        existing.as_ref().and_then(|s| s.dir.clone())
    };

    // session_id: set on --status starting, preserved on subsequent writes
    let session_id = if status == Status::Starting {
        payload.session_id.or_else(|| existing.as_ref().and_then(|s| s.session_id.clone()))
    } else {
        existing.as_ref().and_then(|s| s.session_id.clone())
    };

    // transcript_path: set on starting, preserved on subsequent writes
    let transcript_path = if status == Status::Starting {
        payload.transcript_path.or_else(|| existing.as_ref().and_then(|s| s.transcript_path.clone()))
    } else {
        existing.as_ref().and_then(|s| s.transcript_path.clone())
    };

    let session = Session {
        status,
        tool: None,
        desc: None,
        msg: None,
        ts,
        seq,
        dir,
        session_id,
        transcript_path,
        input_tokens: existing.as_ref().and_then(|s| s.input_tokens),
    };

    registry::write_session_to(registry_dir, session_name, &session)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::App;

    // ── emit_to unit tests ──────────────────────────────────────────

    #[test]
    fn emit_to_writes_session_file() {
        let dir = tempfile::tempdir().unwrap();
        emit_to(dir.path(), "s1", "starting", r#"{"cwd":"/tmp"}"#).unwrap();

        let session = registry::read_session_from(dir.path(), "s1")
            .unwrap()
            .expect("session file should exist");
        assert_eq!(session.status, Status::Starting);
        assert_eq!(session.dir.as_deref(), Some("/tmp"));
        assert_eq!(session.seq, 0);
    }

    #[test]
    fn emit_to_increments_seq() {
        let dir = tempfile::tempdir().unwrap();
        emit_to(dir.path(), "s1", "starting", "{}").unwrap();
        emit_to(dir.path(), "s1", "done", "{}").unwrap();

        let session = registry::read_session_from(dir.path(), "s1")
            .unwrap()
            .unwrap();
        assert_eq!(session.seq, 1);
    }

    #[test]
    fn emit_to_preserves_dir_across_transitions() {
        let dir = tempfile::tempdir().unwrap();
        emit_to(dir.path(), "s1", "starting", r#"{"cwd":"/project"}"#).unwrap();
        emit_to(dir.path(), "s1", "done", "{}").unwrap();

        let session = registry::read_session_from(dir.path(), "s1")
            .unwrap()
            .unwrap();
        assert_eq!(session.dir.as_deref(), Some("/project"));
    }

    #[test]
    fn emit_to_ignores_invalid_status() {
        let dir = tempfile::tempdir().unwrap();
        // Unknown statuses should be silently ignored, not error
        assert!(emit_to(dir.path(), "s1", "bogus", "{}").is_ok());
        // No session file should be written
        let session = registry::read_session_from(dir.path(), "s1").unwrap();
        assert!(session.is_none());
    }

    #[test]
    fn emit_to_with_empty_stdin() {
        let dir = tempfile::tempdir().unwrap();
        emit_to(dir.path(), "s1", "done", "").unwrap();

        let session = registry::read_session_from(dir.path(), "s1")
            .unwrap()
            .unwrap();
        assert_eq!(session.status, Status::Done);
    }

    // ── transcript_path tests ───────────────────────────────────────

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

    // ── session_id tests ────────────────────────────────────────────

    #[test]
    fn emit_starting_parses_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let payload = r#"{"cwd":"/tmp","session_id":"abc123"}"#;
        emit_to(dir.path(), "s1", "starting", payload).unwrap();

        let session = registry::read_session_from(dir.path(), "s1").unwrap().unwrap();
        assert_eq!(session.session_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn emit_preserves_session_id_across_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let payload = r#"{"cwd":"/tmp","session_id":"abc123"}"#;
        emit_to(dir.path(), "s1", "starting", payload).unwrap();
        emit_to(dir.path(), "s1", "done", "{}").unwrap();

        let session = registry::read_session_from(dir.path(), "s1").unwrap().unwrap();
        assert_eq!(session.session_id.as_deref(), Some("abc123"));
    }

    // ── e2e: emit → dashboard visibility ────────────────────────────

    #[test]
    fn emitted_session_appears_in_dashboard() {
        let dir = tempfile::tempdir().unwrap();

        // Simulate the full hook lifecycle (only starting and done are valid)
        emit_to(dir.path(), "trader", "starting", r#"{"cwd":"/home/bob/trade"}"#).unwrap();

        // Dashboard should see the session
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert!(
            app.sessions.contains_key("trader"),
            "dashboard must show the emitted session"
        );
        let session = &app.sessions["trader"];
        assert_eq!(session.status, Status::Starting);
        assert_eq!(session.dir.as_deref(), Some("/home/bob/trade"));
    }

    #[test]
    fn multiple_sessions_appear_in_dashboard_columns() {
        let dir = tempfile::tempdir().unwrap();

        emit_to(dir.path(), "alpha", "starting", r#"{"cwd":"/alpha"}"#).unwrap();
        emit_to(dir.path(), "gamma", "done", "{}").unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions.len(), 2);

        // Check column assignments — Starting status maps to NeedsAttention (waiting for input)
        use crate::dashboard::Column;
        let needs_attention = app.sessions_in_column(Column::NeedsAttention);
        let done = app.sessions_in_column(Column::Done);

        assert_eq!(needs_attention.len(), 1);
        assert_eq!(needs_attention[0].0, "alpha");
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0, "gamma");
    }

    #[test]
    fn session_lifecycle_updates_dashboard() {
        let dir = tempfile::tempdir().unwrap();

        // Start → Done
        emit_to(dir.path(), "s1", "starting", r#"{"cwd":"/proj"}"#).unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions["s1"].status, Status::Starting);

        emit_to(dir.path(), "s1", "done", "{}").unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions["s1"].status, Status::Done);
        // dir should be preserved through the lifecycle
        assert_eq!(app.sessions["s1"].dir.as_deref(), Some("/proj"));
    }

    // ── parser unit tests ───────────────────────────────────────────

    #[test]
    fn parse_status_valid() {
        assert_eq!(parse_status("starting"), Some(Status::Starting));
        assert_eq!(parse_status("done"), Some(Status::Done));
    }

    #[test]
    fn parse_status_unknown_returns_none() {
        assert!(parse_status("bogus").is_none());
        assert!(parse_status("working").is_none());
        assert!(parse_status("waiting").is_none());
        assert!(parse_status("idle").is_none());
    }

    #[test]
    fn parse_payload_session_start() {
        let json = r#"{"cwd": "/home/user/project"}"#;
        let p = parse_stdin_payload(json);
        assert_eq!(p.cwd.as_deref(), Some("/home/user/project"));
    }

    #[test]
    fn parse_payload_transcript_path() {
        let json = r#"{"cwd": "/tmp", "transcript_path": "/path/to/transcript.jsonl"}"#;
        let p = parse_stdin_payload(json);
        assert_eq!(p.transcript_path.as_deref(), Some("/path/to/transcript.jsonl"));
    }

    #[test]
    fn parse_payload_session_id() {
        let json = r#"{"cwd": "/tmp", "session_id": "abc123"}"#;
        let p = parse_stdin_payload(json);
        assert_eq!(p.session_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_payload_empty_stdin() {
        let p = parse_stdin_payload("");
        assert!(p.cwd.is_none());
        assert!(p.transcript_path.is_none());
        assert!(p.session_id.is_none());
    }

    #[test]
    fn parse_payload_invalid_json() {
        let p = parse_stdin_payload("not json at all");
        assert!(p.cwd.is_none());
    }
}
