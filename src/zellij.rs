// Zellij command wrappers: new_tab, go_to_tab, close_tab
//
// All Zellij interaction is isolated here per PRD §9.
// These shell out to `zellij action` subcommands.

#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use std::process::Command;

/// Create a new Zellij tab with the given name, running a command inside it.
///
/// Equivalent to: `zellij action new-tab --name <name> [--cwd <dir>] -- <command> <args...>`
pub fn new_tab(name: &str, command: &str, args: &[&str], cwd: Option<&str>) -> Result<()> {
    let mut cmd = Command::new("zellij");
    cmd.args(["action", "new-tab", "--name", name]);
    if let Some(dir) = cwd {
        cmd.args(["--cwd", dir]);
    }
    cmd.args(["--", command]);
    cmd.args(args);

    let status = cmd
        .status()
        .context("failed to run zellij action new-tab — is zellij installed and running?")?;
    if !status.success() {
        bail!("zellij action new-tab failed with {status}");
    }
    Ok(())
}

/// Switch to the Zellij tab with the given name.
///
/// Equivalent to: `zellij action go-to-tab-name <name>`
pub fn go_to_tab(name: &str) -> Result<()> {
    let status = Command::new("zellij")
        .args(["action", "go-to-tab-name", name])
        .status()
        .context("failed to run zellij action go-to-tab-name — is zellij installed and running?")?;
    if !status.success() {
        bail!("zellij action go-to-tab-name '{name}' failed with {status}");
    }
    Ok(())
}

/// Check whether a Zellij tab with the given name exists.
///
/// Uses `zellij action query-tab-names` and checks if the name appears in the output.
pub fn tab_exists(name: &str) -> Result<bool> {
    let output = Command::new("zellij")
        .args(["action", "query-tab-names"])
        .output()
        .context("failed to run zellij action query-tab-names — is zellij installed and running?")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().any(|line| line.trim() == name))
}

/// Close the Zellij tab with the given name.
///
/// Checks if the tab exists first via `query-tab-names`. If it doesn't exist,
/// returns Ok(()) without closing anything — avoids killing the current tab.
/// (`go-to-tab-name` returns exit 0 for nonexistent tabs, so we can't rely on it.)
pub fn close_tab(name: &str) -> Result<()> {
    // Check if tab exists — go-to-tab-name returns 0 even for missing tabs
    if !tab_exists(name)? {
        return Ok(());
    }

    let _ = go_to_tab(name);

    let status = Command::new("zellij")
        .args(["action", "close-tab"])
        .status()
        .context("failed to run zellij action close-tab — is zellij installed and running?")?;
    if !status.success() {
        bail!("zellij action close-tab failed with {status}");
    }
    Ok(())
}
