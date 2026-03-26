mod dashboard;
mod emit;
pub mod registry;
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
        Commands::Init => todo!("init"),
        Commands::New { name: _ } => todo!("new"),
        Commands::Attach { name: _ } => todo!("attach"),
        Commands::Kill { name: _ } => todo!("kill"),
        Commands::List => todo!("list"),
        Commands::Emit { status: _ } => todo!("emit"),
        Commands::Dashboard => todo!("dashboard"),
    }
}
