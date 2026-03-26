// Zellij command wrappers: new_tab, go_to_tab, close_tab
//
// All Zellij interaction is isolated here per PRD §9.
// These shell out to `zellij action` subcommands.

#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use std::process::Command;

/// Create a new Zellij tab with the given name, running a command inside it.
///
/// Equivalent to: `zellij action new-tab --name <name> -- <command> <args...>`
pub fn new_tab(name: &str, command: &str, args: &[&str]) -> Result<()> {
    let mut cmd = Command::new("zellij");
    cmd.args(["action", "new-tab", "--name", name, "--"]);
    cmd.arg(command);
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

/// Close the Zellij tab with the given name.
///
/// Navigates to the tab first. If the tab doesn't exist (go_to_tab fails),
/// returns Ok(()) without closing anything — avoids killing the current tab.
pub fn close_tab(name: &str) -> Result<()> {
    // Navigate to the tab first — if it doesn't exist, bail early
    if go_to_tab(name).is_err() {
        return Ok(());
    }

    let status = Command::new("zellij")
        .args(["action", "close-tab"])
        .status()
        .context("failed to run zellij action close-tab — is zellij installed and running?")?;
    if !status.success() {
        bail!("zellij action close-tab failed with {status}");
    }
    Ok(())
}
