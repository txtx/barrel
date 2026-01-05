use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "barrel")]
#[command(about = "CLI tool for AI-assisted development workflows")]
#[command(version)]
pub struct Cli {
    /// Shell name to launch (from local barrel.yaml), or "setup" to configure barrel
    #[arg(value_name = "SHELL")]
    pub name: Option<String>,

    /// Path to barrel.yaml manifest file (default: ./barrel.yaml)
    #[arg(
        short = 'm',
        long = "manifest-path",
        value_name = "PATH",
        global = true
    )]
    pub manifest_path: Option<String>,

    /// Terminal profile to use (default: "default")
    #[arg(short = 'p', long = "profile", value_name = "PROFILE")]
    pub profile: Option<String>,

    /// Create a new agent
    #[arg(short = 'n', long = "new", value_name = "AGENT", conflicts_with = "name")]
    pub new_agent: Option<String>,

    /// Kill a workspace session (uses current tmux session if no name given)
    #[arg(
        short = 'k',
        long = "kill",
        value_name = "WORKSPACE",
        num_args = 0..=1,
        default_missing_value = "",
        conflicts_with = "name"
    )]
    pub kill: Option<String>,

    /// Keep generated agent files when killing (don't clean up symlinks)
    #[arg(long = "keep-agents", requires = "kill")]
    pub keep_agents: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a barrel workspace in the current directory
    Init,

    /// Scan for existing agents and consolidate them using AI
    Bootstrap,

    /// Manage agents
    #[command(visible_alias = "agents")]
    Agent {
        #[command(subcommand)]
        action: AgentCommands,
    },
}

#[derive(Subcommand)]
pub enum AgentCommands {
    /// List all available agents (local and global)
    #[command(visible_alias = "ls")]
    List,
    /// Create a new agent
    New {
        /// Name of the agent to create (prompted if not provided)
        name: Option<String>,
    },
    /// Import an agent file to the global agents directory
    Import {
        /// Path to the agent file to import
        path: String,
    },
    /// Fork (copy) a global agent to the current workspace
    Fork {
        /// Name of the global agent to fork
        name: String,
    },
    /// Link (symlink) a global agent to the current workspace
    Link {
        /// Name of the global agent to link
        name: String,
    },
    /// Remove an agent
    Rm {
        /// Name of the agent to remove
        name: String,
    },
}
