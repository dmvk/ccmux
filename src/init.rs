// ccmux init: install Claude Code hooks into ~/.claude/settings.json
//
// Generates the hook configuration per PRD §6, merges with existing settings
// (never overwrites existing hooks), shows a diff, and writes on confirmation.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

/// The ccmux hook entries to install, keyed by hook event name.
/// Uses `$HOME/.cargo/bin/ccmux` so hooks work even when the hook
/// runner's PATH doesn't include ~/.cargo/bin (the shell expands $HOME).
fn ccmux_hooks() -> Vec<(&'static str, &'static str)> {
    vec![
        ("SessionStart", "\"$HOME/.cargo/bin/ccmux\" emit --status starting"),
        ("SessionEnd", "\"$HOME/.cargo/bin/ccmux\" emit --status done"),
    ]
}

/// Build a single hook entry: `{ "type": "command", "command": "<cmd>" }`
fn hook_entry(command: &str) -> Value {
    json!({ "type": "command", "command": command })
}

/// Build a hook matcher entry: `{ "hooks": [{ "type": "command", "command": "<cmd>" }] }`
fn hook_matcher(command: &str) -> Value {
    json!({ "hooks": [hook_entry(command)] })
}

/// Path to `~/.claude/settings.json`
fn settings_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".claude").join("settings.json"))
}

/// Check if the exact command already exists in a hook event's matcher array.
fn has_exact_hook(matchers: &[Value], command: &str) -> bool {
    matchers.iter().any(|matcher| {
        matcher
            .get("hooks")
            .and_then(|h| h.as_array())
            .is_some_and(|hooks| {
                hooks.iter().any(|hook| {
                    hook.get("command").and_then(|c| c.as_str()) == Some(command)
                })
            })
    })
}

/// Merge ccmux hooks into an existing settings Value.
/// Returns the merged settings and whether any changes were made.
fn merge_hooks(settings: &mut Value) -> bool {
    if !settings.is_object() {
        *settings = json!({});
    }
    let Some(obj) = settings.as_object_mut() else {
        return false;
    };

    if !obj.contains_key("hooks") || !obj["hooks"].is_object() {
        obj.insert("hooks".into(), json!({}));
    }
    let Some(hooks_obj) = obj.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
        return false;
    };

    let mut changed = false;

    for (event, command) in ccmux_hooks() {
        let matchers = hooks_obj
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut();

        if let Some(matchers) = matchers
            && !has_exact_hook(matchers, command)
        {
            matchers.push(hook_matcher(command));
            changed = true;
        }
    }

    changed
}

/// Format JSON for display with consistent pretty-printing.
fn format_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// Run the init subcommand.
pub fn run() -> Result<()> {
    let path = settings_path()?;

    // Read existing settings or start with empty object
    let original_text = if path.exists() {
        fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?
    } else {
        "{}".to_string()
    };

    let mut settings: Value = serde_json::from_str(&original_text)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let original = format_json(&settings);

    let changed = merge_hooks(&mut settings);

    if !changed {
        println!("ccmux hooks already installed in {}", path.display());
        return Ok(());
    }

    let updated = format_json(&settings);

    // Show diff
    println!("Changes to {}:\n", path.display());
    for line in diff_lines(&original, &updated) {
        println!("{line}");
    }

    // Confirm
    print!("\nApply these changes? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;

    if !answer.trim().eq_ignore_ascii_case("y") {
        println!("Aborted.");
        return Ok(());
    }

    // Ensure directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(&path, updated.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;

    println!("Hooks installed.");
    Ok(())
}

/// Simple line-by-line diff showing added/removed lines.
fn diff_lines(old: &str, new: &str) -> Vec<String> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut result = Vec::new();

    let mut i = 0;
    let mut j = 0;

    while i < old_lines.len() || j < new_lines.len() {
        if i < old_lines.len() && j < new_lines.len() && old_lines[i] == new_lines[j] {
            result.push(format!("  {}", old_lines[i]));
            i += 1;
            j += 1;
        } else {
            let sync = find_sync(&old_lines, &new_lines, i, j);
            while i < sync.0 {
                result.push(format!("- {}", old_lines[i]));
                i += 1;
            }
            while j < sync.1 {
                result.push(format!("+ {}", new_lines[j]));
                j += 1;
            }
        }
    }

    result
}

/// Find the next point where old and new lines sync up.
fn find_sync(old: &[&str], new: &[&str], oi: usize, ni: usize) -> (usize, usize) {
    for (o, old_line) in old.iter().enumerate().skip(oi) {
        for (n, new_line) in new.iter().enumerate().skip(ni) {
            if old_line == new_line {
                return (o, n);
            }
        }
    }
    (old.len(), new.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_into_empty_settings() {
        let mut settings = json!({});
        let changed = merge_hooks(&mut settings);
        assert!(changed);

        let hooks = settings["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 2);
        assert!(hooks.contains_key("SessionStart"));
        assert!(hooks.contains_key("SessionEnd"));

        // Verify structure of one hook
        let session_start = hooks["SessionStart"].as_array().unwrap();
        assert_eq!(session_start.len(), 1);
        let matcher = &session_start[0];
        let inner = matcher["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["type"], "command");
        assert!(inner[0]["command"].as_str().unwrap().ends_with("ccmux\" emit --status starting"));
    }

    #[test]
    fn merge_preserves_existing_hooks() {
        let mut settings = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "echo pre-bash" }
                        ]
                    }
                ]
            }
        });

        let changed = merge_hooks(&mut settings);
        assert!(changed);

        let session_start = settings["hooks"]["SessionStart"].as_array().unwrap();
        // Should have the existing hook AND the new ccmux hook
        assert_eq!(session_start.len(), 2);
        // Original is preserved
        assert_eq!(session_start[0]["matcher"], "Bash");
        // ccmux hook appended
        let cmd = session_start[1]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("ccmux") && cmd.contains("emit --status starting"));
    }

    #[test]
    fn merge_preserves_existing_settings_keys() {
        let mut settings = json!({
            "permissions": { "allow": ["Read", "Glob"] },
            "hooks": {}
        });

        merge_hooks(&mut settings);

        // permissions key is untouched
        assert_eq!(settings["permissions"]["allow"][0], "Read");
        // hooks are added
        assert!(settings["hooks"].as_object().unwrap().contains_key("SessionStart"));
    }

    #[test]
    fn merge_idempotent() {
        let mut settings = json!({});
        merge_hooks(&mut settings);

        // Second merge should not change anything
        let changed = merge_hooks(&mut settings);
        assert!(!changed);
    }

    #[test]
    fn has_exact_hook_detection() {
        let matchers = vec![hook_matcher("ccmux emit --status starting")];
        assert!(has_exact_hook(&matchers, "ccmux emit --status starting"));
        assert!(!has_exact_hook(&matchers, "ccmux emit --status working"));
    }

    #[test]
    fn hook_entry_structure() {
        let entry = hook_entry("ccmux emit --status working");
        assert_eq!(entry["type"], "command");
        assert_eq!(entry["command"], "ccmux emit --status working");
    }

    #[test]
    fn hook_matcher_structure() {
        let matcher = hook_matcher("ccmux emit --status waiting");
        let hooks = matcher["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "ccmux emit --status waiting");
    }

    #[test]
    fn diff_lines_shows_additions() {
        let old = "{\n}";
        let new = "{\n  \"hooks\": {}\n}";
        let diff = diff_lines(old, new);
        assert!(diff.iter().any(|l| l.starts_with('+')));
    }
}
