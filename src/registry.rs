use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

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
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Returns the registry directory: `~/.ccmux/`
pub fn registry_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".ccmux"))
}

/// Returns the path to a session file: `~/.ccmux/<name>.json`
pub fn session_path(name: &str) -> Result<PathBuf> {
    Ok(registry_dir()?.join(format!("{name}.json")))
}

/// Read a session from a given directory. Returns None if the file doesn't exist.
pub fn read_session_from(dir: &std::path::Path, name: &str) -> Result<Option<Session>> {
    let path = dir.join(format!("{name}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session file: {}", path.display()))?;
    let session: Session = serde_json::from_str(&data)
        .with_context(|| format!("failed to parse session file: {}", path.display()))?;
    Ok(Some(session))
}

/// Read an existing session from the default registry. Returns None if the file doesn't exist.
pub fn read_session(name: &str) -> Result<Option<Session>> {
    read_session_from(&registry_dir()?, name)
}

/// Atomically write a session to a given directory (tmpfile + rename).
pub fn write_session_to(dir: &std::path::Path, name: &str, session: &Session) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let target = dir.join(format!("{name}.json"));
    let tmp = dir.join(format!(".{name}.json.tmp"));

    let json = serde_json::to_string_pretty(session)?;
    let mut file = fs::File::create(&tmp)
        .with_context(|| format!("failed to create temp file: {}", tmp.display()))?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;

    fs::rename(&tmp, &target)
        .with_context(|| format!("failed to rename {} → {}", tmp.display(), target.display()))?;
    Ok(())
}

/// Atomically write a session to the default registry (tmpfile + rename).
pub fn write_session_atomic(name: &str, session: &Session) -> Result<()> {
    write_session_to(&registry_dir()?, name, session)
}

/// Remove a session file from a given directory. Returns Ok(()) even if the file doesn't exist.
pub fn remove_session_from(dir: &std::path::Path, name: &str) -> Result<()> {
    let path = dir.join(format!("{name}.json"));
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to remove session file: {}", path.display())),
    }
}

/// Remove a session file from the default registry directory.
pub fn remove_session(name: &str) -> Result<()> {
    remove_session_from(&registry_dir()?, name)
}

/// List all sessions in a given directory. Returns (name, Session) pairs.
pub fn list_sessions_from(dir: &std::path::Path) -> Result<Vec<(String, Session)>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // Skip temp files
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(String::from);
        if let Some(name) = name {
            let data = fs::read_to_string(&path);
            if let Ok(data) = data
                && let Ok(session) = serde_json::from_str::<Session>(&data)
            {
                sessions.push((name, session));
            }
        }
    }
    sessions.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(sessions)
}

/// List all sessions from the default registry directory.
pub fn list_sessions() -> Result<Vec<(String, Session)>> {
    list_sessions_from(&registry_dir()?)
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
            session_id: None,
        };
        let json = serde_json::to_string(&session).unwrap();
        let deserialized: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.status, session.status);
        assert_eq!(deserialized.tool, session.tool);
        assert_eq!(deserialized.seq, session.seq);
    }

    #[test]
    fn write_and_read_session() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session {
            status: Status::Working,
            tool: Some("Bash".into()),
            msg: None,
            ts: 1711234567,
            seq: 5,
            dir: Some("/home/user/project".into()),
            session_id: None,
        };
        write_session_to(dir.path(), "test", &session).unwrap();
        let read_back = read_session_from(dir.path(), "test").unwrap().unwrap();
        assert_eq!(read_back.status, Status::Working);
        assert_eq!(read_back.tool.as_deref(), Some("Bash"));
        assert_eq!(read_back.seq, 5);
        assert_eq!(read_back.dir.as_deref(), Some("/home/user/project"));
    }

    #[test]
    fn read_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = read_session_from(dir.path(), "nope").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_malformed_json_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.json"), "not valid json").unwrap();
        let result = read_session_from(dir.path(), "bad");
        assert!(result.is_err());
    }

    #[test]
    fn list_sessions_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = list_sessions_from(dir.path()).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_sessions_nonexistent_dir() {
        let dir = std::path::Path::new("/tmp/ccmux-nonexistent-test-dir");
        let sessions = list_sessions_from(dir).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_sessions_multiple() {
        let dir = tempfile::tempdir().unwrap();
        let s1 = Session {
            status: Status::Working,
            tool: Some("Bash".into()),
            msg: None,
            ts: 100,
            seq: 1,
            dir: Some("/project".into()),
            session_id: None,
        };
        let s2 = Session {
            status: Status::Waiting,
            tool: None,
            msg: Some("confirm?".into()),
            ts: 200,
            seq: 3,
            dir: Some("/other".into()),
            session_id: None,
        };
        write_session_to(dir.path(), "alpha", &s1).unwrap();
        write_session_to(dir.path(), "beta", &s2).unwrap();

        let sessions = list_sessions_from(dir.path()).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].0, "alpha");
        assert_eq!(sessions[1].0, "beta");
        assert_eq!(sessions[0].1.status, Status::Working);
        assert_eq!(sessions[1].1.status, Status::Waiting);
    }

    #[test]
    fn list_sessions_skips_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let s1 = Session {
            status: Status::Idle,
            tool: None,
            msg: None,
            ts: 0,
            seq: 0,
            dir: None,
            session_id: None,
        };
        write_session_to(dir.path(), "good", &s1).unwrap();
        std::fs::write(dir.path().join("bad.json"), "not json").unwrap();

        let sessions = list_sessions_from(dir.path()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].0, "good");
    }

    #[test]
    fn list_sessions_skips_non_json_files() {
        let dir = tempfile::tempdir().unwrap();
        let s1 = Session {
            status: Status::Done,
            tool: None,
            msg: None,
            ts: 0,
            seq: 0,
            dir: None,
            session_id: None,
        };
        write_session_to(dir.path(), "real", &s1).unwrap();
        std::fs::write(dir.path().join("readme.txt"), "ignore me").unwrap();

        let sessions = list_sessions_from(dir.path()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].0, "real");
    }

    #[test]
    fn list_sessions_skips_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let s1 = Session {
            status: Status::Working,
            tool: None,
            msg: None,
            ts: 0,
            seq: 0,
            dir: None,
            session_id: None,
        };
        write_session_to(dir.path(), "sess", &s1).unwrap();
        // Simulate a leftover temp file
        std::fs::write(dir.path().join(".sess.json.tmp"), r#"{"status":"idle","tool":null,"msg":null,"ts":0,"seq":0,"dir":null}"#).unwrap();

        let sessions = list_sessions_from(dir.path()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].0, "sess");
    }

    #[test]
    fn atomic_write_no_temp_file_left() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session {
            status: Status::Idle,
            tool: None,
            msg: None,
            ts: 0,
            seq: 0,
            dir: None,
            session_id: None,
        };
        write_session_to(dir.path(), "clean", &session).unwrap();
        // The temp file should be gone after rename
        assert!(!dir.path().join(".clean.json.tmp").exists());
        // The target file should exist
        assert!(dir.path().join("clean.json").exists());
    }

    #[test]
    fn remove_session_existing() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session {
            status: Status::Working,
            tool: None,
            msg: None,
            ts: 0,
            seq: 0,
            dir: None,
            session_id: None,
        };
        write_session_to(dir.path(), "doomed", &session).unwrap();
        assert!(dir.path().join("doomed.json").exists());

        remove_session_from(dir.path(), "doomed").unwrap();
        assert!(!dir.path().join("doomed.json").exists());
    }

    #[test]
    fn remove_session_nonexistent_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        // Should not error when file doesn't exist
        remove_session_from(dir.path(), "ghost").unwrap();
    }
}
