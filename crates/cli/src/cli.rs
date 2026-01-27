//! Command-line interface definitions for axel.
//!
//! This module defines the CLI structure using clap's derive API. Axel supports
//! several modes of operation:
//!
//! - **Workspace mode**: Launch a full tmux workspace from `AXEL.md`
//! - **Shell mode**: Launch a single shell (e.g., `axel claude`)
//! - **Session management**: List, create, and kill tmux sessions
//! - **Skill management**: Create, import, fork, link, and remove skills
//!
//! # Examples
//!
//! ```bash
//! axel                    # Launch workspace from AXEL.md
//! axel claude             # Launch just the claude shell
//! axel -p tmux_cc         # Launch with iTerm2 integration
//! axel -k                 # Kill current workspace
//! axel -w feat/auth       # Create worktree + launch workspace there
//! axel session list       # List running axel sessions
//! axel session new        # Create a new session (same as axel)
//! axel session join foo   # Attach to session "foo"
//! axel session kill foo   # Kill session named "foo"
//! axel skill list         # List available skills
//! axel skill import ./    # Import skills from directory
//! ```

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Axel CLI - AI-assisted development workspace manager.
///
/// Axel provides portable skills across LLMs (Claude Code, Codex, OpenCode)
/// and reproducible terminal workspaces via tmux.
#[derive(Parser)]
#[command(name = "axel")]
#[command(about = "CLI tool for AI-assisted development workflows")]
#[command(version)]
pub struct Cli {
    /// Shell name to launch (from local AXEL.md), or "setup" to configure axel
    #[arg(value_name = "SHELL")]
    pub name: Option<String>,

    /// Path to manifest file (default: ./AXEL.md)
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

    /// Keep generated skill files when killing (don't clean up symlinks)
    #[arg(long = "keep-skills", requires = "kill")]
    pub keep_skills: bool,

    /// Skip confirmation when killing a workspace
    #[arg(long = "confirm", requires = "kill")]
    pub confirm: bool,

    /// Send a prompt to an existing tmux pane instead of launching a new shell.
    ///
    /// Use with --prompt to send text to the specified pane.
    /// The pane ID can be a tmux pane identifier (e.g., %5) or target format.
    #[arg(long = "pane-id", value_name = "PANE")]
    pub pane_id: Option<String>,

    /// Port for the axel event server (hooks and OTEL telemetry).
    ///
    /// When specified with --pane-id, configures Claude hooks and OTEL
    /// endpoints to use this port instead of the default 4318.
    #[arg(long = "port", value_name = "PORT")]
    pub server_port: Option<u16>,

    /// Prompt text to send to the shell.
    ///
    /// When used with --pane-id, sends the prompt to the existing pane.
    /// When used with a shell name (e.g., `axel claude --prompt '...'`),
    /// overrides the prompt defined in AXEL.md.
    #[arg(long = "prompt", value_name = "TEXT")]
    pub prompt: Option<String>,

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

/// Top-level subcommands for axel.
#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a axel workspace in the current directory.
    ///
    /// Creates `AXEL.md` with a default configuration and an `skills/`
    /// directory with an `index.md` template for project documentation.
    Init,

    /// Scan for existing skills and consolidate them using AI.
    ///
    /// Discovers skill files across your filesystem (Claude, Codex, OpenCode formats)
    /// and uses an AI assistant to merge and organize them into `~/.config/axel/skills/`.
    /// This is experimental; prefer `axel skill import` for controlled imports.
    Bootstrap,

    /// Manage skills (create, import, fork, link, remove).
    ///
    /// Skills are markdown files with system prompts that axel automatically
    /// installs to each AI tool's expected location (symlinks for Claude/OpenCode,
    /// merged file for Codex).
    #[command(visible_alias = "skills")]
    Skill {
        #[command(subcommand)]
        action: SkillCommands,
    },

    /// Manage tmux sessions (list, create, kill).
    ///
    /// Sessions are tmux workspaces created by axel. Use these commands
    /// to list running sessions, create new ones, or kill existing ones.
    #[command(visible_alias = "sessions")]
    Session {
        #[command(subcommand)]
        action: SessionCommands,
    },

    /// Run the axel event server.
    ///
    /// Starts an HTTP server that receives Claude Code hook events and OTEL
    /// telemetry data. The server broadcasts events via SSE and logs them
    /// to a JSONL file. Required for the Axel macOS app to receive events.
    Server {
        /// Port to listen on
        #[arg(short, long, default_value = "4318")]
        port: u16,

        /// Tmux session name to monitor for auto-shutdown (optional)
        #[arg(short, long)]
        session: Option<String>,

        /// Path to the JSONL log file
        #[arg(short, long, default_value = ".axel/events.jsonl")]
        log: PathBuf,
    },
}

/// Skill management subcommands.
///
/// Skills can exist in two locations:
/// - **Local**: `./skills/` in the current workspace (higher precedence)
/// - **Global**: `~/.config/axel/skills/` (shared across workspaces)
#[derive(Subcommand)]
pub enum SkillCommands {
    /// List all available skills (local and global).
    ///
    /// Shows skill name, location, and description. Local skills override
    /// global skills with the same name.
    #[command(visible_alias = "ls")]
    List,

    /// Create a new skill interactively.
    ///
    /// Prompts for location (local or global) and opens the new skill
    /// file in your `$EDITOR`.
    New {
        /// Name of the skill to create (prompted if not provided)
        name: Option<String>,
    },

    /// Import skill file(s) to the global skills directory.
    ///
    /// Accepts a single `.md` file or a directory containing multiple skills.
    /// Each skill is stored as `~/.config/axel/skills/<name>/AGENT.md`.
    Import {
        /// Path to the skill file or directory to import
        path: String,
    },

    /// Fork (copy) a global skill to the current workspace.
    ///
    /// Creates an independent copy in `./skills/<name>/AGENT.md` that you
    /// can modify without affecting the global version.
    Fork {
        /// Name of the global skill to fork
        name: String,
    },

    /// Link (symlink) a global skill to the current workspace.
    ///
    /// Creates a symlink from `./skills/<name>/` to the global skill.
    /// Changes to the global skill will be reflected in the workspace.
    Link {
        /// Name of the global skill to link
        name: String,
    },

    /// Remove an skill.
    ///
    /// If the skill exists in both local and global locations, prompts
    /// for which one to remove.
    Rm {
        /// Name of the skill to remove
        name: String,
    },
}

/// Session management subcommands.
///
/// Manage axel tmux sessions - list running workspaces, create new ones,
/// or kill existing sessions.
#[derive(Subcommand)]
pub enum SessionCommands {
    /// List all running axel sessions.
    ///
    /// Shows session name, working directory, window count, and attachment status.
    /// Use `--all` to include non-axel tmux sessions.
    #[command(visible_alias = "ls")]
    List {
        /// Show all tmux sessions, not just axel sessions
        #[arg(short, long)]
        all: bool,
    },

    /// Create a new workspace session.
    ///
    /// Equivalent to running `axel` or `axel <shell>`. Launches a workspace
    /// from the AXEL.md manifest in the current directory.
    New {
        /// Shell name to launch (from AXEL.md), or launches full workspace if omitted
        shell: Option<String>,
    },

    /// Join (attach to) an existing session.
    ///
    /// Attaches to a running axel or tmux session. If already inside tmux,
    /// switches to the target session.
    Join {
        /// Name of the session to join
        name: String,
    },

    /// Kill a running workspace session.
    ///
    /// Equivalent to `axel -k <name>`. Terminates all panes, closes the tmux
    /// session, and cleans up skill symlinks.
    Kill {
        /// Name of the session to kill (uses current session if omitted)
        name: Option<String>,

        /// Keep skill symlinks instead of cleaning them up
        #[arg(long)]
        keep_skills: bool,

        /// Skip confirmation prompt
        #[arg(long = "confirm")]
        confirm: bool,
    },
}
