// ccmux emit: stdin parsing, atomic writes to session registry
//
// Called by Claude Code hooks. Reads CCMUX_SESSION env var for the session name,
// parses the hook JSON payload from stdin, and atomically writes the session file.

use anyhow::{bail, Result};
use std::io::{IsTerminal, Read};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::registry::{self, Session, Status};

/// Parse a --status flag value into a Status enum.
fn parse_status(s: &str) -> Result<Status> {
    match s {
        "starting" => Ok(Status::Starting),
        "working" => Ok(Status::Working),
        "waiting" => Ok(Status::Waiting),
        "idle" => Ok(Status::Idle),
        "done" => Ok(Status::Done),
        _ => bail!("unknown status: {s}"),
    }
}

/// Extract fields from the Claude Code hook stdin JSON payload.
struct HookPayload {
    tool_name: Option<String>,
    message: Option<String>,
    cwd: Option<String>,
}

fn parse_stdin_payload(stdin_data: &str) -> HookPayload {
    let val: serde_json::Value = serde_json::from_str(stdin_data).unwrap_or_default();

    let tool_name = val
        .get("tool_name")
        .and_then(|v| v.as_str())
        .map(String::from);

    let message = val
        .get("message")
        .and_then(|v| v.as_str())
        .map(|s| truncate(s, 80).to_string());

    let cwd = val
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(String::from);

    HookPayload {
        tool_name,
        message,
        cwd,
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        // Find a char boundary at or before max
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
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
    let status = parse_status(status_str)?;
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

    // tool: only populated when working
    let tool = if status == Status::Working {
        payload.tool_name
    } else {
        None
    };

    // msg: only populated when waiting
    let msg = if status == Status::Waiting {
        payload.message
    } else {
        None
    };

    let session = Session {
        status,
        tool,
        msg,
        ts,
        seq,
        dir,
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
        emit_to(dir.path(), "s1", "working", r#"{"tool_name":"Bash"}"#).unwrap();

        let session = registry::read_session_from(dir.path(), "s1")
            .unwrap()
            .unwrap();
        assert_eq!(session.seq, 1);
    }

    #[test]
    fn emit_to_preserves_dir_across_transitions() {
        let dir = tempfile::tempdir().unwrap();
        emit_to(dir.path(), "s1", "starting", r#"{"cwd":"/project"}"#).unwrap();
        emit_to(dir.path(), "s1", "working", r#"{"tool_name":"Edit"}"#).unwrap();

        let session = registry::read_session_from(dir.path(), "s1")
            .unwrap()
            .unwrap();
        assert_eq!(session.dir.as_deref(), Some("/project"));
    }

    #[test]
    fn emit_to_rejects_invalid_status() {
        let dir = tempfile::tempdir().unwrap();
        assert!(emit_to(dir.path(), "s1", "bogus", "{}").is_err());
    }

    #[test]
    fn emit_to_with_empty_stdin() {
        let dir = tempfile::tempdir().unwrap();
        emit_to(dir.path(), "s1", "idle", "").unwrap();

        let session = registry::read_session_from(dir.path(), "s1")
            .unwrap()
            .unwrap();
        assert_eq!(session.status, Status::Idle);
    }

    // ── e2e: emit → dashboard visibility ────────────────────────────

    #[test]
    fn emitted_session_appears_in_dashboard() {
        let dir = tempfile::tempdir().unwrap();

        // Simulate the full hook lifecycle
        emit_to(dir.path(), "trader", "starting", r#"{"cwd":"/home/bob/trade"}"#).unwrap();
        emit_to(dir.path(), "trader", "working", r#"{"tool_name":"Bash"}"#).unwrap();

        // Dashboard should see the session
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert!(
            app.sessions.contains_key("trader"),
            "dashboard must show the emitted session"
        );
        let session = &app.sessions["trader"];
        assert_eq!(session.status, Status::Working);
        assert_eq!(session.tool.as_deref(), Some("Bash"));
        assert_eq!(session.dir.as_deref(), Some("/home/bob/trade"));
    }

    #[test]
    fn multiple_sessions_appear_in_dashboard_columns() {
        let dir = tempfile::tempdir().unwrap();

        emit_to(dir.path(), "alpha", "working", r#"{"tool_name":"Edit"}"#).unwrap();
        emit_to(dir.path(), "beta", "waiting", r#"{"message":"Approve?"}"#).unwrap();
        emit_to(dir.path(), "gamma", "done", "{}").unwrap();

        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions.len(), 3);

        // Check column assignments
        use crate::dashboard::Column;
        let working = app.sessions_in_column(Column::Working);
        let waiting = app.sessions_in_column(Column::Waiting);
        let done = app.sessions_in_column(Column::Done);

        assert_eq!(working.len(), 1);
        assert_eq!(working[0].0, "alpha");
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].0, "beta");
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0, "gamma");
    }

    #[test]
    fn session_lifecycle_updates_dashboard() {
        let dir = tempfile::tempdir().unwrap();

        // Start → Working → Waiting → Idle → Done
        emit_to(dir.path(), "s1", "starting", r#"{"cwd":"/proj"}"#).unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions["s1"].status, Status::Starting);

        emit_to(dir.path(), "s1", "working", r#"{"tool_name":"Bash"}"#).unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions["s1"].status, Status::Working);
        assert_eq!(app.sessions["s1"].tool.as_deref(), Some("Bash"));

        emit_to(dir.path(), "s1", "waiting", r#"{"message":"Continue?"}"#).unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions["s1"].status, Status::Waiting);
        assert_eq!(app.sessions["s1"].msg.as_deref(), Some("Continue?"));

        emit_to(dir.path(), "s1", "idle", "{}").unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions["s1"].status, Status::Idle);

        emit_to(dir.path(), "s1", "done", "{}").unwrap();
        let app = App::with_registry_dir(dir.path()).unwrap();
        assert_eq!(app.sessions["s1"].status, Status::Done);
        // dir should be preserved through the entire lifecycle
        assert_eq!(app.sessions["s1"].dir.as_deref(), Some("/proj"));
    }

    // ── parser unit tests ───────────────────────────────────────────

    #[test]
    fn parse_status_valid() {
        assert_eq!(parse_status("starting").unwrap(), Status::Starting);
        assert_eq!(parse_status("working").unwrap(), Status::Working);
        assert_eq!(parse_status("waiting").unwrap(), Status::Waiting);
        assert_eq!(parse_status("idle").unwrap(), Status::Idle);
        assert_eq!(parse_status("done").unwrap(), Status::Done);
    }

    #[test]
    fn parse_status_invalid() {
        assert!(parse_status("bogus").is_err());
    }

    #[test]
    fn parse_payload_pretooluse() {
        let json = r#"{"tool_name": "Edit", "tool_input": {}}"#;
        let p = parse_stdin_payload(json);
        assert_eq!(p.tool_name.as_deref(), Some("Edit"));
        assert!(p.message.is_none());
    }

    #[test]
    fn parse_payload_notification() {
        let json = r#"{"message": "Should I increase position size?"}"#;
        let p = parse_stdin_payload(json);
        assert!(p.tool_name.is_none());
        assert_eq!(p.message.as_deref(), Some("Should I increase position size?"));
    }

    #[test]
    fn parse_payload_session_start() {
        let json = r#"{"cwd": "/home/user/project"}"#;
        let p = parse_stdin_payload(json);
        assert_eq!(p.cwd.as_deref(), Some("/home/user/project"));
    }

    #[test]
    fn parse_payload_empty_stdin() {
        let p = parse_stdin_payload("");
        assert!(p.tool_name.is_none());
        assert!(p.message.is_none());
        assert!(p.cwd.is_none());
    }

    #[test]
    fn parse_payload_invalid_json() {
        let p = parse_stdin_payload("not json at all");
        assert!(p.tool_name.is_none());
    }

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hello", 80), "hello");
    }

    #[test]
    fn truncate_long() {
        let long = "a".repeat(100);
        assert_eq!(truncate(&long, 80).len(), 80);
    }

    #[test]
    fn truncate_multibyte() {
        // "café" is 5 bytes (é is 2 bytes)
        let s = "café";
        let t = truncate(s, 4);
        assert!(t.len() <= 4);
        assert_eq!(t, "caf"); // cuts before the multi-byte char
    }
}
