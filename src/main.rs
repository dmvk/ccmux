mod dashboard;
mod emit;
mod init;
pub mod registry;
mod transcript;
mod ui;
mod zellij;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ccmux", version, about = "Claude Code session multiplexer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Install hooks into ~/.claude/settings.json
    Init,
    /// Create a new session in a Zellij tab
    New {
        /// Session name (alphanumeric + hyphen, max 20 chars)
        name: String,
    },
    /// Attach to an existing session tab
    Attach {
        /// Session name
        name: String,
    },
    /// Kill a session and remove its registry file
    Kill {
        /// Session name
        name: String,
    },
    /// List all sessions
    List,
    /// Write session status (called by hooks, not users)
    Emit {
        /// Session status
        #[arg(long)]
        status: String,
    },
    /// Launch the TUI dashboard
    Dashboard,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => init::run(),
        Commands::New { ref name } => {
            registry::validate_session_name(name)?;
            if registry::read_session(name)?.is_some() {
                anyhow::bail!("session '{name}' already exists");
            }
            let env_var = format!("CCMUX_SESSION={name}");
            zellij::new_tab(name, "env", &[&env_var, "claude", "--dangerously-skip-permissions", "--worktree"], None)?;
            Ok(())
        }
        Commands::Attach { ref name } => zellij::go_to_tab(name),
        Commands::Kill { ref name } => {
            // Close Zellij tab (best-effort — tab may not exist if not in Zellij)
            let _ = zellij::close_tab(name);
            registry::remove_session(name)?;
            Ok(())
        }
        Commands::List => {
            let sessions = registry::list_sessions()?;
            if sessions.is_empty() {
                println!("No sessions.");
                return Ok(());
            }
            // Header
            println!(
                "{:<20} {:<10} {:<12} {:<40} DIR",
                "NAME", "STATUS", "TOOL", "MESSAGE"
            );
            println!("{}", "-".repeat(90));
            for (name, s) in &sessions {
                let status = match s.status {
                    registry::Status::Starting => "starting",
                    registry::Status::Working => "working",
                    registry::Status::Waiting => "waiting",
                    registry::Status::Idle => "idle",
                    registry::Status::Done => "done",
                };
                let tool = s.tool.as_deref().unwrap_or("");
                let msg = s.msg.as_deref().unwrap_or("");
                let dir = s.dir.as_deref().unwrap_or("");
                println!("{:<20} {:<10} {:<12} {:<40} {}", name, status, tool, msg, dir);
            }
            Ok(())
        }
        Commands::Emit { ref status } => emit::run(status),
        Commands::Dashboard => dashboard::run(),
    }
}
