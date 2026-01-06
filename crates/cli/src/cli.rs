//! Command-line interface definitions for barrel.
//!
//! This module defines the CLI structure using clap's derive API. Barrel supports
//! several modes of operation:
//!
//! - **Workspace mode**: Launch a full tmux workspace from `barrel.yaml`
//! - **Shell mode**: Launch a single shell (e.g., `barrel claude`)
//! - **Session management**: List, create, and kill tmux sessions
//! - **Agent management**: Create, import, fork, link, and remove agents
//!
//! # Examples
//!
//! ```bash
//! barrel                    # Launch workspace from barrel.yaml
//! barrel claude             # Launch just the claude shell
//! barrel -p tmux_cc         # Launch with iTerm2 integration
//! barrel -k                 # Kill current workspace
//! barrel -w feat/auth       # Create worktree + launch workspace there
//! barrel session list       # List running barrel sessions
//! barrel session new        # Create a new session (same as barrel)
//! barrel session join foo   # Attach to session "foo"
//! barrel session kill foo   # Kill session named "foo"
//! barrel agent list         # List available agents
//! barrel agent import ./    # Import agents from directory
//! ```

use clap::{Parser, Subcommand};

/// Barrel CLI - AI-assisted development workspace manager.
///
/// Barrel provides portable agents across LLMs (Claude Code, Codex, OpenCode)
/// and reproducible terminal workspaces via tmux.
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

    /// Skip confirmation when killing a workspace
    #[arg(long = "confirm", requires = "kill")]
    pub confirm: bool,

    /// Create/use git worktree for branch and launch workspace from there.
    ///
    /// If the branch doesn't exist, it will be created from the default branch.
    /// The worktree is created as a sibling directory to the repository.
    #[arg(short = 'w', long = "worktree", value_name = "BRANCH")]
    pub worktree: Option<String>,

    /// Remove the git worktree when killing the workspace (use with -k)
    #[arg(long = "prune", requires = "kill")]
    pub prune_worktree: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Top-level subcommands for barrel.
#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a barrel workspace in the current directory.
    ///
    /// Creates `barrel.yaml` with a default configuration and an `agents/`
    /// directory with an `index.md` template for project documentation.
    Init,

    /// Scan for existing agents and consolidate them using AI.
    ///
    /// Discovers agent files across your filesystem (Claude, Codex, OpenCode formats)
    /// and uses an AI assistant to merge and organize them into `~/.config/barrel/agents/`.
    /// This is experimental; prefer `barrel agent import` for controlled imports.
    Bootstrap,

    /// Manage agents (create, import, fork, link, remove).
    ///
    /// Agents are markdown files with system prompts that barrel automatically
    /// installs to each AI tool's expected location (symlinks for Claude/OpenCode,
    /// merged file for Codex).
    #[command(visible_alias = "agents")]
    Agent {
        #[command(subcommand)]
        action: AgentCommands,
    },

    /// Manage tmux sessions (list, create, kill).
    ///
    /// Sessions are tmux workspaces created by barrel. Use these commands
    /// to list running sessions, create new ones, or kill existing ones.
    #[command(visible_alias = "sessions")]
    Session {
        #[command(subcommand)]
        action: SessionCommands,
    },
}

/// Agent management subcommands.
///
/// Agents can exist in two locations:
/// - **Local**: `./agents/` in the current workspace (higher precedence)
/// - **Global**: `~/.config/barrel/agents/` (shared across workspaces)
#[derive(Subcommand)]
pub enum AgentCommands {
    /// List all available agents (local and global).
    ///
    /// Shows agent name, location, and description. Local agents override
    /// global agents with the same name.
    #[command(visible_alias = "ls")]
    List,

    /// Create a new agent interactively.
    ///
    /// Prompts for location (local or global) and opens the new agent
    /// file in your `$EDITOR`.
    New {
        /// Name of the agent to create (prompted if not provided)
        name: Option<String>,
    },

    /// Import agent file(s) to the global agents directory.
    ///
    /// Accepts a single `.md` file or a directory containing multiple agents.
    /// Each agent is stored as `~/.config/barrel/agents/<name>/AGENT.md`.
    Import {
        /// Path to the agent file or directory to import
        path: String,
    },

    /// Fork (copy) a global agent to the current workspace.
    ///
    /// Creates an independent copy in `./agents/<name>/AGENT.md` that you
    /// can modify without affecting the global version.
    Fork {
        /// Name of the global agent to fork
        name: String,
    },

    /// Link (symlink) a global agent to the current workspace.
    ///
    /// Creates a symlink from `./agents/<name>/` to the global agent.
    /// Changes to the global agent will be reflected in the workspace.
    Link {
        /// Name of the global agent to link
        name: String,
    },

    /// Remove an agent.
    ///
    /// If the agent exists in both local and global locations, prompts
    /// for which one to remove.
    Rm {
        /// Name of the agent to remove
        name: String,
    },
}

/// Session management subcommands.
///
/// Manage barrel tmux sessions - list running workspaces, create new ones,
/// or kill existing sessions.
#[derive(Subcommand)]
pub enum SessionCommands {
    /// List all running barrel sessions.
    ///
    /// Shows session name, working directory, window count, and attachment status.
    /// Use `--all` to include non-barrel tmux sessions.
    #[command(visible_alias = "ls")]
    List {
        /// Show all tmux sessions, not just barrel sessions
        #[arg(short, long)]
        all: bool,
    },

    /// Create a new workspace session.
    ///
    /// Equivalent to running `barrel` or `barrel <shell>`. Launches a workspace
    /// from the barrel.yaml manifest in the current directory.
    New {
        /// Shell name to launch (from barrel.yaml), or launches full workspace if omitted
        shell: Option<String>,
    },

    /// Join (attach to) an existing session.
    ///
    /// Attaches to a running barrel or tmux session. If already inside tmux,
    /// switches to the target session.
    Join {
        /// Name of the session to join
        name: String,
    },

    /// Kill a running workspace session.
    ///
    /// Equivalent to `barrel -k <name>`. Terminates all panes, closes the tmux
    /// session, and cleans up agent symlinks.
    Kill {
        /// Name of the session to kill (uses current session if omitted)
        name: Option<String>,

        /// Keep agent symlinks instead of cleaning them up
        #[arg(long)]
        keep_agents: bool,

        /// Skip confirmation prompt
        #[arg(long = "confirm")]
        confirm: bool,
    },
}
