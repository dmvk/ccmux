// ccmux emit: stdin parsing, atomic writes to session registry
//
// Called by Claude Code hooks. Reads CCMUX_SESSION env var for the session name,
// parses the hook JSON payload from stdin, and atomically writes the session file.

use anyhow::{bail, Result};
use std::io::Read;
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

    let status = parse_status(status_str)?;

    // Read stdin (hook JSON payload)
    let mut stdin_data = String::new();
    std::io::stdin().read_to_string(&mut stdin_data)?;
    let payload = parse_stdin_payload(&stdin_data);

    // Read existing session to carry forward seq and dir
    let existing = registry::read_session(&session_name)?;

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

    registry::write_session_atomic(&session_name, &session)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
