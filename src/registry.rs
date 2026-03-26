use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Starting,
    Working,
    Waiting,
    Idle,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub status: Status,
    pub tool: Option<String>,
    pub msg: Option<String>,
    pub ts: u64,
    pub seq: u64,
    pub dir: Option<String>,
}

/// Validate a session name per PRD §5:
/// - Non-empty
/// - Max 20 characters
/// - Only `[a-zA-Z0-9-]`
pub fn validate_session_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("session name cannot be empty");
    }
    if name.len() > 20 {
        bail!(
            "session name too long ({} chars, max 20): {name}",
            name.len()
        );
    }
    if let Some(c) = name.chars().find(|c| !c.is_ascii_alphanumeric() && *c != '-') {
        bail!("session name contains invalid character '{c}': only [a-zA-Z0-9-] allowed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names() {
        assert!(validate_session_name("trading").is_ok());
        assert!(validate_session_name("ml-feats").is_ok());
        assert!(validate_session_name("A-1-b-2").is_ok());
        assert!(validate_session_name("a").is_ok());
        assert!(validate_session_name("12345678901234567890").is_ok()); // exactly 20
    }

    #[test]
    fn rejects_empty() {
        let err = validate_session_name("").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn rejects_too_long() {
        let err = validate_session_name("aaaaaaaaaaaaaaaaaaaaa").unwrap_err(); // 21 chars
        assert!(err.to_string().contains("too long"));
    }

    #[test]
    fn rejects_spaces() {
        let err = validate_session_name("bad name").unwrap_err();
        assert!(err.to_string().contains("invalid character"));
    }

    #[test]
    fn rejects_special_chars() {
        let err = validate_session_name("bad!name").unwrap_err();
        assert!(err.to_string().contains("invalid character"));
    }

    #[test]
    fn rejects_underscore() {
        let err = validate_session_name("bad_name").unwrap_err();
        assert!(err.to_string().contains("invalid character"));
    }

    #[test]
    fn session_roundtrip_serde() {
        let session = Session {
            status: Status::Working,
            tool: Some("Edit".into()),
            msg: None,
            ts: 1711234567,
            seq: 42,
            dir: Some("~/project".into()),
        };
        let json = serde_json::to_string(&session).unwrap();
        let deserialized: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.status, session.status);
        assert_eq!(deserialized.tool, session.tool);
        assert_eq!(deserialized.seq, session.seq);
    }
}
