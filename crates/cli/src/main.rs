//! Barrel CLI - AI-assisted development workspace manager.
//!
//! Barrel provides portable agents across LLMs (Claude Code, Codex, OpenCode) and
//! reproducible terminal workspaces via tmux. Write agents once and use them with
//! any supported AI assistant.
//!
//! # Architecture
//!
//! The CLI handles:
//! - **Manifest resolution**: Walks up the directory tree to find `barrel.yaml`
//! - **Profile selection**: Chooses between `tmux`, `tmux_cc` (iTerm2), or `shell` modes
//! - **Agent installation**: Delegates to drivers in `barrel-core` for each AI tool
//! - **Session management**: Creates, attaches to, and kills tmux sessions
//!
//! # Workflow
//!
//! 1. User runs `barrel` in a project directory
//! 2. CLI finds `barrel.yaml` by walking up the directory tree
//! 3. Profile type determines launch mode (tmux, iTerm2, or single shell)
//! 4. Agents are installed via drivers (symlinks for Claude/OpenCode, merged for Codex)
//! 5. Tmux session is created with configured panes, or shell is exec'd directly
//!
//! Core functionality (config parsing, drivers, tmux commands) is in `barrel-core`.

mod cli;

use std::path::{Path, PathBuf};

use anyhow::Result;
use barrel_core::{
    ClaudeDriver, ProfileType, ShellConfig,
    claude::ClaudeCommand,
    config::{expand_path, generate_config, load_config, workspaces_dir},
    drivers,
    tmux::{
        BARREL_MANIFEST_ENV, attach_session, create_workspace as tmux_create_workspace,
        current_session, detach_session, get_environment, has_session, kill_session,
    },
};
use clap::{CommandFactory, Parser};
use cli::{AgentCommands, Cli, Commands};
use colored::Colorize;

// =============================================================================
// Path Constants
// =============================================================================

const AGENT_FILE: &str = "AGENT.md";
const AGENTS_DIR: &str = "agents";
const BARREL_DIR: &str = "barrel";
const CONFIG_DIR: &str = ".config";

// =============================================================================
// Main Entry Point
// =============================================================================

/// Entry point for the barrel CLI.
///
/// Parses command-line arguments and dispatches to the appropriate handler:
///
/// - **Subcommands** (`init`, `bootstrap`, `agent`): Handled first
/// - **Flags** (`-n`, `-k`): Create agent or kill workspace
/// - **Shell name**: Launch specific shell from manifest (e.g., `barrel claude`)
/// - **No args**: Launch full workspace from `barrel.yaml` or show help
///
/// The manifest path is resolved by walking up the directory tree from the
/// current directory until `barrel.yaml` is found, or uses the path specified
/// with `-m/--manifest-path`.
fn main() -> Result<()> {
    let cli = Cli::parse();
    let workspaces_dir = workspaces_dir();
    let manifest_path = resolve_manifest_path(cli.manifest_path.as_deref());
    let base_dir = manifest_base_dir(&manifest_path);

    // Handle subcommands first
    if let Some(command) = cli.command {
        return match command {
            Commands::Init => init_workspace(),
            Commands::Bootstrap => bootstrap_agents(),
            Commands::Agent { action } => match action {
                AgentCommands::List => list_agents(&manifest_path, &base_dir),
                AgentCommands::New { name } => new_agent(name.as_deref(), &base_dir),
                AgentCommands::Import { path } => import_agent(&path),
                AgentCommands::Fork { name } => fork_agent(&name, &manifest_path, &base_dir),
                AgentCommands::Link { name } => link_agent(&name, &manifest_path, &base_dir),
                AgentCommands::Rm { name } => rm_agent(&name, &manifest_path, &base_dir),
            },
        };
    }

    if let Some(name) = cli.new_agent {
        create_agent(&name)?;
    } else if let Some(name) = cli.kill {
        let session_name = if name.is_empty() {
            // No workspace specified, try to detect current tmux session
            current_session().ok_or_else(|| {
                anyhow::anyhow!(
                    "Not inside a tmux session. Specify a workspace name: barrel -k <workspace>"
                )
            })?
        } else {
            name
        };
        do_kill_workspace(&workspaces_dir, &session_name, cli.keep_agents)?;
    } else if let Some(ref name) = cli.name {
        if name == "setup" {
            setup_barrel()?;
        } else if manifest_path.exists() {
            launch_shell_by_name(&manifest_path, name)?;
        } else {
            eprintln!(
                "{} No barrel.yaml found. Run '{}' to create one.",
                "✘".red(),
                "barrel init".blue()
            );
            std::process::exit(1);
        }
    } else if cli.manifest_path.is_some() || manifest_path.exists() {
        launch_from_manifest(&manifest_path, cli.profile.as_deref())?;
    } else {
        Cli::command().print_help()?;
    }

    Ok(())
}

// =============================================================================
// Path Resolution
// =============================================================================

/// Resolve manifest path from CLI option or default to ./barrel.yaml
fn resolve_manifest_path(cli_path: Option<&str>) -> PathBuf {
    if let Some(p) = cli_path {
        let path = PathBuf::from(p);
        return path.canonicalize().unwrap_or(path);
    }

    // Walk up directory tree looking for barrel.yaml
    let mut current = std::env::current_dir().unwrap_or_default();
    loop {
        let candidate = current.join("barrel.yaml");
        if candidate.exists() {
            return candidate.canonicalize().unwrap_or(candidate);
        }

        match current.parent() {
            Some(parent) if parent != current => {
                current = parent.to_path_buf();
            }
            _ => break,
        }
    }

    std::env::current_dir()
        .unwrap_or_default()
        .join("barrel.yaml")
}

/// Get the base directory (parent of manifest) for resolving relative paths
fn manifest_base_dir(manifest_path: &Path) -> PathBuf {
    manifest_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))
}

/// Convert absolute path to display path (replace home with ~)
fn display_path(path: &Path) -> String {
    dirs::home_dir()
        .and_then(|home| {
            path.strip_prefix(&home)
                .ok()
                .map(|rel| Path::new("~").join(rel).display().to_string())
        })
        .unwrap_or_else(|| path.display().to_string())
}

// =============================================================================
// Workspace Commands
// =============================================================================

/// Initialize a barrel workspace in the current directory
fn init_workspace() -> Result<()> {
    use dialoguer::{Input, theme::ColorfulTheme};

    let current_dir = std::env::current_dir()?;
    let config_path = current_dir.join("barrel.yaml");

    if config_path.exists() {
        eprintln!("{}", "barrel.yaml already exists in this directory".red());
        std::process::exit(1);
    }

    let theme = ColorfulTheme::default();

    // Default name from directory
    let default_name = current_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let name: String = Input::with_theme(&theme)
        .with_prompt("Workspace name")
        .default(default_name)
        .interact_text()?;

    // Create agents directory
    let agents_dir = current_dir.join("agents");
    if !agents_dir.exists() {
        std::fs::create_dir_all(&agents_dir)?;
        println!("{} {} agents/", "✔".green(), "Created".dimmed());
    }

    // Create index.md template
    let index_path = agents_dir.join("index.md");
    if !index_path.exists() {
        let index_content = format!(
            r#"---
name: {name}
description: Project documentation for AI assistants
---

# {name}

## Overview

<!-- Brief description of what this project does -->

## Getting Started

<!-- How to set up and run the project -->

## Architecture

<!-- High-level architecture overview -->

## Key Files

<!-- Important files and what they contain -->
"#,
            name = name
        );
        std::fs::write(&index_path, index_content)?;
        println!("{} {} agents/index.md", "✔".green(), "Created".dimmed());
    }

    // Create barrel.yaml
    let yaml_content = generate_config(&name, &current_dir.to_string_lossy());
    std::fs::write(&config_path, yaml_content)?;
    println!("{} {} barrel.yaml", "✔".green(), "Created".dimmed());

    println!();
    println!("Launch with: {}", "barrel".blue());

    Ok(())
}

/// Scan for existing agents and consolidate them using AI.
///
/// This experimental command discovers agent files across the filesystem by:
/// 1. Prompting for a directory to scan
/// 2. Walking the directory tree (ignoring .gitignore) looking for known agent patterns
/// 3. Copying found files to a staging directory (`~/.config/barrel/agents/.bootstrap-staging/`)
/// 4. Launching an AI assistant to consolidate and organize the agents
///
/// The AI is given instructions to merge duplicates, create proper directory structures
/// (`<name>/AGENT.md`), and clean up the staging directory when done.
///
/// For more controlled imports, prefer `barrel agent import`.
fn bootstrap_agents() -> Result<()> {
    use std::os::unix::process::CommandExt;

    use barrel_core::all_agent_patterns;
    use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
    use ignore::WalkBuilder;

    let theme = ColorfulTheme::default();
    let current_dir = std::env::current_dir()?;

    // Prompt for directory to scan
    let scan_dir: String = Input::with_theme(&theme)
        .with_prompt("Directory to scan for agents")
        .default(current_dir.to_string_lossy().to_string())
        .interact_text()?;

    // Expand ~ to home directory
    let expanded_dir = if let Some(rest) = scan_dir.strip_prefix("~/") {
        home_dir()?.join(rest).to_string_lossy().to_string()
    } else {
        scan_dir.clone()
    };

    let scan_path = PathBuf::from(&expanded_dir);
    if !scan_path.exists() {
        eprintln!("{}", format!("Directory not found: {}", expanded_dir).red());
        std::process::exit(1);
    }

    println!();
    println!(
        "{} Scanning {} for agent files...",
        "...".dimmed(),
        scan_dir
    );
    println!();

    // Get agent file patterns from all drivers
    let agent_patterns = all_agent_patterns();

    // Use ignore crate (ripgrep's directory walker) for fast traversal
    // Don't respect .gitignore since agent files are often gitignored
    let mut found_agents: Vec<PathBuf> = Vec::new();

    let walker = WalkBuilder::new(&scan_path)
        .hidden(false) // Include hidden directories like .claude
        .git_ignore(false) // Don't respect .gitignore - agent files are often ignored
        .git_global(false)
        .git_exclude(false)
        .build();

    for entry in walker.flatten() {
        let path = entry.path();

        // Skip if not a file or if it's a symlink
        let metadata = match path.symlink_metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }

        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let path_str = path.to_string_lossy();

        // Check if this matches any of our patterns
        let is_agent = agent_patterns.iter().any(|pattern| {
            if pattern.contains('*') {
                // Simple glob matching
                let parts: Vec<&str> = pattern.split('*').collect();
                if parts.len() == 2 {
                    let prefix = parts[0];
                    let suffix = parts[1];
                    path_str.contains(prefix.trim_start_matches('.')) && path_str.ends_with(suffix)
                } else {
                    false
                }
            } else {
                file_name == *pattern || path_str.ends_with(pattern)
            }
        });

        if is_agent {
            found_agents.push(path.to_path_buf());
        }
    }

    if found_agents.is_empty() {
        println!("{}", "No agent files found.".yellow());
        return Ok(());
    }

    // Display found agents
    println!("{} {} agent files:", "✔".green(), found_agents.len());
    println!();
    for agent in &found_agents {
        let rel_path = agent
            .strip_prefix(&scan_path)
            .unwrap_or(agent)
            .display()
            .to_string();
        println!("  {} {}", "-".dimmed(), rel_path);
    }
    println!();

    // Confirm consolidation
    let proceed = Confirm::with_theme(&theme)
        .with_prompt("Consolidate these agents to ~/.config/barrel/agents?")
        .default(true)
        .interact()?;

    if !proceed {
        println!("{}", "Cancelled".dimmed());
        return Ok(());
    }

    // Copy agents to global directory
    let global_agents_dir = home_dir()?.join(".config/barrel/agents");
    std::fs::create_dir_all(&global_agents_dir)?;

    let staging_dir = global_agents_dir.join(".bootstrap-staging");
    std::fs::create_dir_all(&staging_dir)?;

    for (i, agent_path) in found_agents.iter().enumerate() {
        let dest_name = format!(
            "{:03}-{}.md",
            i,
            agent_path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("agent")
        );
        let dest_path = staging_dir.join(&dest_name);
        std::fs::copy(agent_path, &dest_path)?;
    }

    println!();
    println!(
        "{} {} files to ~/.config/barrel/agents/.bootstrap-staging/",
        "✔".green(),
        found_agents.len()
    );

    // Select AI for consolidation
    let ai_options = ["Claude Code", "Codex", "OpenCode"];
    let ai_selection = Select::with_theme(&theme)
        .with_prompt("Which AI should consolidate these agents?")
        .items(&ai_options)
        .default(0)
        .interact()?;

    let ai_command = match ai_selection {
        0 => "claude",
        1 => "codex",
        2 => "opencode",
        _ => unreachable!(),
    };

    println!();
    println!(
        "{} Starting {} to consolidate agents...",
        "✔".green(),
        ai_command
    );
    println!();

    // Build the consolidation prompt
    let prompt = format!(
        r#"I have {} agent files in .bootstrap-staging/ that were discovered from various projects.

Please consolidate and organize them into clean agents:

## Instructions

1. Read all files in .bootstrap-staging/
2. Identify unique, valuable agents (merge duplicates, remove redundant ones)
3. For each unique agent, create a directory structure:

   <agent-name>/AGENT.md

   Example: code-reviewer/AGENT.md, rust-developer/AGENT.md

4. Each AGENT.md must have this format:
   ```markdown
   ---
   name: <agent-name>
   description: <one-line description of what this agent does>
   ---

   # <Agent Name>

   <agent instructions here>
   ```

5. After creating all agents, delete the .bootstrap-staging/ directory

## Guidelines

- Use kebab-case for directory names (e.g., code-reviewer, not CodeReviewer)
- Merge similar agents into one comprehensive agent
- Focus on quality over quantity
- Keep the best instructions from duplicates"#,
        found_agents.len()
    );

    // Change to the global agents directory and launch AI
    std::env::set_current_dir(&global_agents_dir)?;

    let err = std::process::Command::new(ai_command).arg(&prompt).exec();

    Err(err.into())
}

fn do_kill_workspace(workspaces_dir: &Path, name: &str, keep_agents: bool) -> Result<()> {
    if has_session(name) {
        let session_manifest = get_environment(name, BARREL_MANIFEST_ENV).map(PathBuf::from);

        let cleaned = if !keep_agents {
            let config_path = workspaces_dir.join(name).join("barrel.yaml");
            let local_config = std::env::current_dir().ok().map(|d| d.join("barrel.yaml"));

            let cfg = session_manifest
                .and_then(|p| load_config(&p).ok())
                .or_else(|| load_config(&config_path).ok())
                .or_else(|| local_config.and_then(|p| load_config(&p).ok()));

            cfg.and_then(|c| c.workspace_dir())
                .map(|dir| cleanup_agents(&dir))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        detach_session(name)?;
        kill_session(name)?;

        println!("{} {} {}", "✔".green(), "Killed workspace".dimmed(), name);

        if !cleaned.is_empty() {
            println!(
                "{} {} {} agents",
                "✔".green(),
                "Cleaned".dimmed(),
                format_cleaned_drivers(&cleaned)
            );
        }
    } else {
        eprintln!("{}", format!("No running session: {}", name).red());
    }
    Ok(())
}

/// Launch a workspace from a manifest file.
///
/// This is the main launch path when running `barrel` with a `barrel.yaml` present.
///
/// # Session handling
///
/// If a tmux session already exists with the workspace name:
/// - Verifies the session belongs to the same manifest (via `BARREL_MANIFEST` env var)
/// - If it's the same workspace, attaches to the existing session
/// - If it's a different workspace, errors with guidance to rename the workspace
///
/// # Profile types
///
/// The profile type (from `barrel.yaml` or `-p` flag) determines the launch mode:
/// - `shell`: No tmux, exec's the first shell directly (single pane)
/// - `tmux_cc`: iTerm2 integration via `tmux -CC`
/// - `tmux`: Standard tmux with pane layout
fn launch_from_manifest(config_path: &Path, profile: Option<&str>) -> Result<()> {
    if !config_path.exists() {
        eprintln!(
            "{}",
            format!("Manifest not found: {}", config_path.display()).red()
        );
        std::process::exit(1);
    }

    let session_name = config_path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let config = load_config(config_path)?;
    let profile_type = config.profile_type(profile);

    if !session_name.is_empty() && has_session(&session_name) {
        // Check if this session belongs to a different workspace
        let current_manifest = config_path
            .canonicalize()
            .unwrap_or_else(|_| config_path.to_path_buf());

        if let Some(existing_manifest) = get_environment(&session_name, BARREL_MANIFEST_ENV) {
            let existing_path = PathBuf::from(&existing_manifest);
            if existing_path != current_manifest {
                eprintln!(
                    "{} A session named '{}' already exists for a different workspace:",
                    "✘".red(),
                    session_name
                );
                eprintln!(
                    "  {} {}",
                    "existing:".dimmed(),
                    display_path(&existing_path)
                );
                eprintln!(
                    "  {} {}",
                    "current: ".dimmed(),
                    display_path(&current_manifest)
                );
                eprintln!();
                eprintln!(
                    "{}",
                    "To fix this, update the 'workspace' field in your barrel.yaml to use a unique name.".yellow()
                );
                std::process::exit(1);
            }
        }

        println!(
            "{}",
            format!("Attaching to existing session: {}", session_name).blue()
        );
        return match profile_type {
            ProfileType::TmuxCC => {
                std::process::Command::new("tmux")
                    .args(["-CC", "attach-session", "-t", &session_name])
                    .status()?;
                Ok(())
            }
            _ => attach_session(&session_name),
        };
    }

    match profile_type {
        ProfileType::Shell => launch_shell_mode(&config, profile),
        ProfileType::TmuxCC => launch_tmux_cc_mode(config_path, &config, profile),
        ProfileType::Tmux => launch_tmux_mode(&config, profile),
    }
}

/// Launch in shell mode (no tmux, just run the first shell).
///
/// Used when the profile type is `shell`. This mode:
/// 1. Resolves the first pane from the profile
/// 2. Installs agents for the appropriate driver (Claude, Codex, or OpenCode)
/// 3. Creates CLAUDE.md symlink if using Claude
/// 4. Builds the command and exec's it (replacing the barrel process)
///
/// This is useful for single-tool workflows or when tmux isn't desired.
fn launch_shell_mode(config: &barrel_core::WorkspaceConfig, profile: Option<&str>) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let panes = config.resolve_panes(profile);
    let index = config.load_index();

    if panes.is_empty() {
        anyhow::bail!("No shells defined in profile");
    }

    let first_pane = &panes[0];

    let work_dir = first_pane
        .path()
        .map(|p| PathBuf::from(expand_path(p)))
        .or_else(|| config.workspace_dir());

    if let Some(ref workspace_dir) = work_dir {
        let (driver_name, agent_names) = match &first_pane.config {
            ShellConfig::Claude(c) => ("claude", &c.agents),
            ShellConfig::Codex(c) => ("codex", &c.agents),
            ShellConfig::Opencode(c) => ("opencode", &c.agents),
            ShellConfig::Custom(_) => ("", &Vec::new()),
        };

        if !agent_names.is_empty()
            && let Some(driver) = drivers::get_driver(driver_name)
        {
            let agent_paths = config.resolve_agents(agent_names);
            if let Some(count) = driver
                .install_agents(workspace_dir, &agent_paths)
                .ok()
                .filter(|&c| c > 0)
            {
                let agents_word = if count == 1 { "agent" } else { "agents" };
                eprintln!(
                    "{} {} {} {} for {}",
                    "✔".green(),
                    "Installed".dimmed(),
                    count,
                    agents_word,
                    driver.name()
                );
            }
        }

        if matches!(&first_pane.config, ShellConfig::Claude(_)) {
            let claude_driver = ClaudeDriver;
            if claude_driver
                .install_index(config, workspace_dir)
                .unwrap_or(false)
            {
                eprintln!("{} {} CLAUDE.md symlink", "✔".green(), "Created".dimmed());
            }
        }
    }

    let command = match &first_pane.config {
        ShellConfig::Claude(c) => {
            let mut cmd = ClaudeCommand::new();
            if let Some(model) = &c.model {
                cmd = cmd.model(model);
            }
            if !c.allowed_tools.is_empty() {
                cmd = cmd.allowed_tools(c.allowed_tools.clone());
            }
            if !c.disallowed_tools.is_empty() {
                cmd = cmd.disallowed_tools(c.disallowed_tools.clone());
            }
            if let Some(prompt) = &c.prompt {
                cmd = cmd.prompt(prompt);
            }
            for arg in &c.args {
                cmd = cmd.extra_arg(arg);
            }
            Some(cmd.build())
        }
        ShellConfig::Codex(c) => {
            let mut parts = vec!["codex".to_string()];
            if let Some(model) = &c.model {
                parts.push("-m".to_string());
                parts.push(model.clone());
            }
            for arg in &c.args {
                parts.push(arg.clone());
            }
            if let Some(prompt) = &c.prompt {
                let escaped = prompt.replace('\'', "'\\''");
                parts.push(format!("'{}'", escaped));
            } else if let Some(ref idx) = index {
                let escaped = idx.to_initial_prompt().replace('\'', "'\\''");
                parts.push(format!("'{}'", escaped));
            }
            Some(parts.join(" "))
        }
        ShellConfig::Opencode(c) => {
            let mut parts = vec!["opencode".to_string()];
            if let Some(model) = &c.model {
                parts.push("-m".to_string());
                parts.push(model.clone());
            }
            for arg in &c.args {
                parts.push(arg.clone());
            }
            if let Some(prompt) = &c.prompt {
                let escaped = prompt.replace('\'', "'\\''");
                parts.push(format!("'{}'", escaped));
            } else if let Some(ref idx) = index {
                let escaped = idx.to_initial_prompt().replace('\'', "'\\''");
                parts.push(format!("'{}'", escaped));
            }
            Some(parts.join(" "))
        }
        ShellConfig::Custom(c) => c.command.clone(),
    };

    if let Some(ref dir) = work_dir {
        std::env::set_current_dir(dir)?;
    }

    if let Some(cmd) = command {
        let err = std::process::Command::new("sh").arg("-c").arg(&cmd).exec();
        Err(err.into())
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        let err = std::process::Command::new(&shell).exec();
        Err(err.into())
    }
}

/// Launch a specific shell by name from the manifest.
///
/// Used when running `barrel <shell_name>` (e.g., `barrel claude`). This:
/// 1. Loads the manifest and finds the shell config matching the name
/// 2. Installs agents for the shell's driver type
/// 3. Builds and runs the command in the current terminal
/// 4. Cleans up agent symlinks when the shell exits
///
/// Unlike `launch_shell_mode`, this runs the command in a subprocess (not exec)
/// so cleanup can happen after the shell exits.
fn launch_shell_by_name(manifest_path: &Path, shell_name: &str) -> Result<()> {
    let config = load_config(manifest_path)?;
    let index = config.load_index();

    let shell_config = config
        .shells
        .iter()
        .find(|s| s.shell_type() == shell_name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Shell '{}' not found in manifest. Available shells: {}",
                shell_name,
                config
                    .shells
                    .iter()
                    .map(|s| s.shell_type())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

    let current_dir = std::env::current_dir().ok();

    if let Some(ref install_dir) = current_dir {
        let (driver_name, agent_names) = match shell_config {
            ShellConfig::Claude(c) => ("claude", &c.agents),
            ShellConfig::Codex(c) => ("codex", &c.agents),
            ShellConfig::Opencode(c) => ("opencode", &c.agents),
            ShellConfig::Custom(_) => ("", &Vec::new()),
        };

        if !agent_names.is_empty()
            && let Some(driver) = drivers::get_driver(driver_name)
        {
            let agent_paths = config.resolve_agents(agent_names);
            if let Some(count) = driver
                .install_agents(install_dir, &agent_paths)
                .ok()
                .filter(|&c| c > 0)
            {
                let agents_word = if count == 1 { "agent" } else { "agents" };
                eprintln!(
                    "{} {} {} {} for {}",
                    "✔".green(),
                    "Installed".dimmed(),
                    count,
                    agents_word,
                    driver.name()
                );
            }
        }

        if matches!(shell_config, ShellConfig::Claude(_)) {
            let claude_driver = ClaudeDriver;
            if claude_driver
                .install_index(&config, install_dir)
                .unwrap_or(false)
            {
                eprintln!("{} {} CLAUDE.md symlink", "✔".green(), "Created".dimmed());
            }
        }
    }

    let command = match shell_config {
        ShellConfig::Claude(c) => {
            let mut cmd = ClaudeCommand::new();
            if let Some(model) = &c.model {
                cmd = cmd.model(model);
            }
            if !c.allowed_tools.is_empty() {
                cmd = cmd.allowed_tools(c.allowed_tools.clone());
            }
            if !c.disallowed_tools.is_empty() {
                cmd = cmd.disallowed_tools(c.disallowed_tools.clone());
            }
            if let Some(prompt) = &c.prompt {
                cmd = cmd.prompt(prompt);
            }
            for arg in &c.args {
                cmd = cmd.extra_arg(arg);
            }
            Some(cmd.build())
        }
        ShellConfig::Codex(c) => {
            let mut parts = vec!["codex".to_string()];
            if let Some(model) = &c.model {
                parts.push("-m".to_string());
                parts.push(model.clone());
            }
            for arg in &c.args {
                parts.push(arg.clone());
            }
            if let Some(prompt) = &c.prompt {
                let escaped = prompt.replace('\'', "'\\''");
                parts.push(format!("'{}'", escaped));
            } else if let Some(ref idx) = index {
                let escaped = idx.to_initial_prompt().replace('\'', "'\\''");
                parts.push(format!("'{}'", escaped));
            }
            Some(parts.join(" "))
        }
        ShellConfig::Opencode(c) => {
            let mut parts = vec!["opencode".to_string()];
            if let Some(model) = &c.model {
                parts.push("-m".to_string());
                parts.push(model.clone());
            }
            for arg in &c.args {
                parts.push(arg.clone());
            }
            if let Some(prompt) = &c.prompt {
                let escaped = prompt.replace('\'', "'\\''");
                parts.push(format!("'{}'", escaped));
            } else if let Some(ref idx) = index {
                let escaped = idx.to_initial_prompt().replace('\'', "'\\''");
                parts.push(format!("'{}'", escaped));
            }
            Some(parts.join(" "))
        }
        ShellConfig::Custom(c) => c.command.clone(),
    };

    let status = if let Some(cmd) = command {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .status()
    } else {
        eprintln!("{}", "No command built, falling back to shell".red());
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        std::process::Command::new(&shell).status()
    };

    if let Some(ref install_dir) = current_dir {
        let cleaned = cleanup_agents(install_dir);
        if !cleaned.is_empty() {
            eprintln!(
                "{} {} {} artifacts",
                "✔".green(),
                "Cleaned".dimmed(),
                format_cleaned_drivers(&cleaned)
            );
        }
    }

    status?;
    Ok(())
}

/// Launch in tmux control mode (-CC) for iTerm2 integration.
///
/// iTerm2 supports tmux control mode which allows it to manage tmux panes
/// as native iTerm2 tabs/splits. This provides a more integrated experience
/// on macOS with features like native scrollback and mouse support.
///
/// The session is created normally, then attached with `tmux -CC attach-session`.
fn launch_tmux_cc_mode(
    config_path: &Path,
    config: &barrel_core::WorkspaceConfig,
    profile: Option<&str>,
) -> Result<()> {
    let session_name = config_path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| config.workspace.clone());

    if has_session(&session_name) {
        println!(
            "{}",
            format!("Attaching to existing session (CC mode): {}", session_name).blue()
        );
        std::process::Command::new("tmux")
            .args(["-CC", "attach-session", "-t", &session_name])
            .status()?;
        return Ok(());
    }

    tmux_create_workspace(&session_name, config, profile)?;
    println!(
        "{} {} {}",
        "✔".green(),
        "Created tmux session (CC mode)".dimmed(),
        config.workspace
    );

    std::process::Command::new("tmux")
        .args(["-CC", "attach-session", "-t", &session_name])
        .status()?;

    Ok(())
}

/// Launch in standard tmux mode.
///
/// Creates a tmux session with the configured pane layout. If a session
/// with the workspace name already exists, attaches to it instead.
///
/// The session includes:
/// - Mouse support and clipboard integration
/// - Pane border titles showing shell names
/// - Barrel-styled status bar with version info
/// - Automatic agent installation for each AI pane
fn launch_tmux_mode(config: &barrel_core::WorkspaceConfig, profile: Option<&str>) -> Result<()> {
    let session_name = config
        .manifest_path
        .as_ref()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| config.workspace.clone());

    if has_session(&session_name) {
        println!(
            "{}",
            format!("Attaching to existing session: {}", session_name).blue()
        );
        attach_session(&session_name)?;
        return Ok(());
    }

    tmux_create_workspace(&session_name, config, profile)?;
    println!(
        "{} {} {}",
        "✔".green(),
        "Created tmux session".dimmed(),
        config.workspace
    );
    attach_session(&session_name)?;

    Ok(())
}

// =============================================================================
// Setup Command
// =============================================================================

fn setup_barrel() -> Result<()> {
    use dialoguer::{Input, theme::ColorfulTheme};

    let theme = ColorfulTheme::default();
    let home = home_dir()?;

    println!("{}", "Barrel Setup".blue().bold());
    println!();

    let global_dir = home.join(".barrel");
    let global_agents = global_dir.join("agents");

    if !global_dir.exists() {
        std::fs::create_dir_all(&global_dir)?;
        println!("{} {} ~/.barrel/", "✔".green(), "Created".dimmed());
    }

    if !global_agents.exists() {
        std::fs::create_dir_all(&global_agents)?;
        println!("{} {} ~/.barrel/agents/", "✔".green(), "Created".dimmed());
    }

    let global_config = global_dir.join("barrel.yaml");
    if !global_config.exists() {
        std::fs::write(&global_config, "# Global barrel configuration\n")?;
        println!(
            "{} {} ~/.barrel/barrel.yaml",
            "✔".green(),
            "Created".dimmed()
        );
    }

    println!();

    let setup_org: String = Input::with_theme(&theme)
        .with_prompt("Organization name (leave empty to skip)")
        .allow_empty(true)
        .interact_text()?;

    if !setup_org.is_empty() {
        let org_base: String = Input::with_theme(&theme)
            .with_prompt("Organization base path")
            .default(format!("{}/Coding/{}", home.display(), setup_org))
            .interact_text()?;

        let org_path = PathBuf::from(&org_base);
        let org_agents = org_path.join("agents");
        let org_workspaces = org_path.join("workspaces");

        if !org_path.exists() {
            std::fs::create_dir_all(&org_path)?;
            println!("{} {} {}/", "✔".green(), "Created".dimmed(), org_base);
        }

        if !org_agents.exists() {
            std::fs::create_dir_all(&org_agents)?;
            println!(
                "{} {} {}/agents/",
                "✔".green(),
                "Created".dimmed(),
                org_base
            );
        }

        if !org_workspaces.exists() {
            std::fs::create_dir_all(&org_workspaces)?;
            println!(
                "{} {} {}/workspaces/",
                "✔".green(),
                "Created".dimmed(),
                org_base
            );
        }
    }

    println!();
    println!("{}", "Setup complete!".green().bold());

    Ok(())
}

// =============================================================================
// Agent Commands
// =============================================================================

fn create_agent(name: &str) -> Result<()> {
    use dialoguer::{Select, theme::ColorfulTheme};

    let theme = ColorfulTheme::default();
    let current_dir = std::env::current_dir()?;
    let home = home_dir()?;

    let workspace_agents = current_dir.join("agents");
    let global_agents = home.join(".barrel").join("agents");

    let mut options = Vec::new();
    let mut paths = Vec::new();

    if workspace_agents.exists() || current_dir.join("barrel.yaml").exists() {
        options.push("Workspace (./agents/)".to_string());
        paths.push(workspace_agents.clone());
    }

    options.push("Global (~/.barrel/agents/)".to_string());
    paths.push(global_agents.clone());

    let selection = if options.len() == 1 {
        0
    } else {
        Select::with_theme(&theme)
            .with_prompt("Where should this agent be created?")
            .items(&options)
            .default(0)
            .interact()?
    };

    let target_dir = &paths[selection];

    std::fs::create_dir_all(target_dir)?;

    let agent_path = target_dir.join(format!("{}.md", name));

    if agent_path.exists() {
        eprintln!("{}", format!("Agent '{}' already exists", name).red());
        std::process::exit(1);
    }

    let agent_content = format!(
        "# {}\n\nDescribe what this agent does and how it should behave.\n",
        name
    );
    std::fs::write(&agent_path, agent_content)?;

    let display_path = if target_dir == &global_agents {
        format!("~/.barrel/agents/{}.md", name)
    } else {
        format!("agents/{}.md", name)
    };
    println!("{} {} {}", "✔".green(), "Created".dimmed(), display_path);

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "code".to_string());
    std::process::Command::new(editor)
        .arg(&agent_path)
        .status()?;

    Ok(())
}

/// Clean up installed agent symlinks for all drivers
fn cleanup_agents(workspace_dir: &Path) -> Vec<&'static str> {
    let mut cleaned = Vec::new();

    for driver in drivers::all_drivers() {
        if driver.cleanup(workspace_dir) {
            cleaned.push(driver.name());
        }
    }

    cleaned
}

/// Format cleaned drivers list for display
fn format_cleaned_drivers(cleaned: &[&str]) -> String {
    if cleaned.len() == 1 {
        cleaned[0].to_string()
    } else {
        let last = cleaned.last().unwrap();
        let rest = &cleaned[..cleaned.len() - 1];
        format!("{} and {}", rest.join(", "), last)
    }
}

// =============================================================================
// Agent Path Helpers
// =============================================================================

fn global_agents_dir() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(CONFIG_DIR)
        .join(BARREL_DIR)
        .join(AGENTS_DIR))
}

/// Represents an agent's location in the filesystem.
///
/// Agents follow the convention `<base>/<name>/AGENT.md` where:
/// - Local agents: `./agents/<name>/AGENT.md`
/// - Global agents: `~/.config/barrel/agents/<name>/AGENT.md`
///
/// This struct provides methods for checking existence, getting file paths,
/// and formatting display strings for user output.
struct AgentPath {
    /// Directory containing the AGENT.md file
    dir: PathBuf,
    /// Whether this is a global agent (affects display formatting)
    is_global: bool,
}

impl AgentPath {
    fn local(name: &str, base_dir: &Path) -> Self {
        Self {
            dir: base_dir.join(AGENTS_DIR).join(name),
            is_global: false,
        }
    }

    fn global(name: &str) -> Result<Self> {
        Ok(Self {
            dir: global_agents_dir()?.join(name),
            is_global: true,
        })
    }

    fn exists(&self) -> bool {
        self.dir.exists()
    }

    fn agent_file(&self) -> PathBuf {
        self.dir.join(AGENT_FILE)
    }

    fn display(&self) -> String {
        if self.is_global {
            display_path(&self.dir)
        } else {
            Path::new(AGENTS_DIR)
                .join(self.dir.file_name().unwrap_or_default())
                .display()
                .to_string()
        }
    }

    fn display_with_file(&self) -> String {
        if self.is_global {
            display_path(&self.agent_file())
        } else {
            Path::new(AGENTS_DIR)
                .join(self.dir.file_name().unwrap_or_default())
                .join(AGENT_FILE)
                .display()
                .to_string()
        }
    }
}

/// Get all global agent directories to search
fn global_agent_dirs() -> Vec<PathBuf> {
    global_agents_dir()
        .ok()
        .filter(|p| p.exists())
        .into_iter()
        .collect()
}

/// Metadata for a discovered agent, used for listing.
///
/// Contains the agent's name, a description extracted from the file content,
/// and location information for display purposes.
struct AgentInfo {
    /// Agent name (directory name or file stem)
    name: String,
    /// First non-empty, non-heading line from the agent file (truncated to 60 chars)
    description: String,
    /// Full path to the agent file
    #[allow(dead_code)]
    path: PathBuf,
    /// Location label for display (workspace name or "global")
    location: String,
}

/// Find all agents in a directory.
///
/// Discovers agents in two formats:
/// - Directory format: `<name>/AGENT.md`
/// - File format: `<name>.md` (excluding `index.md`)
///
/// For each agent, extracts a description from the file content by finding
/// the first non-empty line that isn't a heading (or falls back to the first heading).
fn find_agents_in_dir(dir: &Path, location: &str) -> Vec<AgentInfo> {
    let mut agents = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return agents,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        let (agent_name, agent_path) = if path.is_dir() {
            let agent_file = path.join("AGENT.md");
            if agent_file.exists() {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                (name, agent_file)
            } else {
                continue;
            }
        } else if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            if path.file_name().is_some_and(|n| n == "index.md") {
                continue;
            }
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            (name, path)
        } else {
            continue;
        };

        if agent_name.is_empty() {
            continue;
        }

        let description = std::fs::read_to_string(&agent_path)
            .ok()
            .and_then(|content| {
                let content = if content.starts_with("---") {
                    content
                        .find("\n---")
                        .map(|i| &content[i + 4..])
                        .unwrap_or(&content)
                } else {
                    &content
                };

                content
                    .lines()
                    .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
                    .or_else(|| {
                        content
                            .lines()
                            .find(|l| l.starts_with('#'))
                            .map(|l| l.trim_start_matches('#').trim())
                    })
                    .map(|s| {
                        let s = s.trim();
                        if s.len() > 60 {
                            format!("{}...", &s[..57])
                        } else {
                            s.to_string()
                        }
                    })
            })
            .unwrap_or_else(|| "No description".to_string());

        agents.push(AgentInfo {
            name: agent_name,
            description,
            path: agent_path,
            location: location.to_string(),
        });
    }

    agents
}

fn list_agents(manifest_path: &Path, base_dir: &Path) -> Result<()> {
    let mut all_agents: Vec<AgentInfo> = Vec::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    let global_dir = global_agents_dir().ok();

    let agent_sources: Vec<(PathBuf, String)> = if manifest_path.exists() {
        let cfg = load_config(manifest_path)?;
        cfg.agents_dirs()
            .into_iter()
            .map(|dir| {
                let name = if dir.starts_with(base_dir) {
                    base_dir
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "local".to_string())
                } else if global_dir.as_ref().is_some_and(|g| &dir == g) {
                    "global".to_string()
                } else {
                    display_path(&dir)
                };
                (dir, name)
            })
            .collect()
    } else {
        let mut sources = Vec::new();
        let local_dir = base_dir.join(AGENTS_DIR);
        if local_dir.exists() {
            let name = base_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "local".to_string());
            sources.push((local_dir, name));
        }
        for dir in global_agent_dirs() {
            sources.push((dir, "global".to_string()));
        }
        sources
    };

    for (dir, location) in &agent_sources {
        for agent in find_agents_in_dir(dir, location) {
            if !seen_names.contains(&agent.name) {
                seen_names.insert(agent.name.clone());
                all_agents.push(agent);
            }
        }
    }

    if all_agents.is_empty() {
        println!("{}", "No agents found".dimmed());
        return Ok(());
    }

    use comfy_table::{Table, presets::NOTHING};

    let workspace_name = base_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut table = Table::new();
    table.load_preset(NOTHING);

    for agent in &all_agents {
        let location = if agent.location == workspace_name {
            agent.location.yellow().to_string()
        } else {
            agent.location.purple().to_string()
        };

        table.add_row(vec![
            agent.name.green().to_string(),
            location,
            agent.description.dimmed().to_string(),
        ]);
    }

    println!("{table}");

    Ok(())
}

fn fork_agent(name: &str, manifest_path: &Path, base_dir: &Path) -> Result<()> {
    let global = AgentPath::global(name)?;
    let local = AgentPath::local(name, base_dir);

    if !global.exists() {
        eprintln!("{}", format!("Global agent '{}' not found", name).red());
        eprintln!();
        let _ = list_agents(manifest_path, base_dir);
        std::process::exit(1);
    }

    if local.exists() {
        eprintln!(
            "{}",
            format!("Agent '{}' already exists in workspace", name).red()
        );
        std::process::exit(1);
    }

    std::fs::create_dir_all(&local.dir)?;
    std::fs::copy(global.agent_file(), local.agent_file())?;

    println!(
        "{} {} {}",
        "✔".green(),
        "Forked".dimmed(),
        local.display_with_file()
    );

    Ok(())
}

fn link_agent(name: &str, manifest_path: &Path, base_dir: &Path) -> Result<()> {
    let global = AgentPath::global(name)?;
    let local = AgentPath::local(name, base_dir);

    if !global.exists() {
        eprintln!("{}", format!("Global agent '{}' not found", name).red());
        eprintln!();
        let _ = list_agents(manifest_path, base_dir);
        std::process::exit(1);
    }

    if local.exists() {
        eprintln!(
            "{}",
            format!("Agent '{}' already exists in workspace", name).red()
        );
        std::process::exit(1);
    }

    std::fs::create_dir_all(base_dir.join(AGENTS_DIR))?;

    #[cfg(unix)]
    std::os::unix::fs::symlink(&global.dir, &local.dir)?;

    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&global.dir, &local.dir)?;

    println!(
        "{} {} {} -> {}",
        "✔".green(),
        "Linked".dimmed(),
        local.display(),
        global.display()
    );

    Ok(())
}

fn new_agent(name: Option<&str>, base_dir: &Path) -> Result<()> {
    use dialoguer::{Input, Select, theme::ColorfulTheme};

    let theme = ColorfulTheme::default();

    let agent_name: String = match name {
        Some(n) => n.to_string(),
        None => Input::with_theme(&theme)
            .with_prompt("Agent name")
            .interact_text()?,
    };

    let local = AgentPath::local(&agent_name, base_dir);
    let global = AgentPath::global(&agent_name)?;

    let options = [
        format!("Local ({})", local.display()),
        format!("Global ({})", global.display()),
    ];
    let selection = Select::with_theme(&theme)
        .with_prompt("Where should this agent be created?")
        .items(&options)
        .default(0)
        .interact()?;

    let agent = match selection {
        0 => local,
        1 => global,
        _ => unreachable!(),
    };

    if agent.exists() {
        let collision_options = ["Replace", "Cancel"];
        let collision_selection = Select::with_theme(&theme)
            .with_prompt(format!("Agent '{}' already exists", agent_name))
            .items(&collision_options)
            .default(1)
            .interact()?;

        match collision_selection {
            0 => {
                std::fs::remove_dir_all(&agent.dir)?;
            }
            1 => {
                println!("{}", "Cancelled".dimmed());
                return Ok(());
            }
            _ => unreachable!(),
        }
    }

    std::fs::create_dir_all(&agent.dir)?;

    let content = format!(
        r#"---
name: {name}
description: Describe what this agent does
---

# {name}

You are a {name} agent.

## Guidelines

- Add your guidelines here
"#,
        name = agent_name
    );
    let agent_file = agent.agent_file();

    std::fs::write(&agent_file, content)?;

    println!(
        "{} {} {}",
        "✔".green(),
        "Created".dimmed(),
        agent.display_with_file()
    );

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "code".to_string());
    std::process::Command::new(editor)
        .arg(&agent_file)
        .status()?;

    Ok(())
}

fn import_agent(path: &str) -> Result<()> {
    // Expand ~ to home directory
    let expanded_path = if let Some(rest) = path.strip_prefix("~/") {
        home_dir()?.join(rest)
    } else {
        PathBuf::from(path)
    };

    if !expanded_path.exists() {
        eprintln!("{} Path not found: {}", "✘".red(), path);
        std::process::exit(1);
    }

    // Skip symlinks
    let metadata = expanded_path.symlink_metadata()?;
    if metadata.file_type().is_symlink() {
        eprintln!("{} Cannot import symlinks", "✘".red());
        std::process::exit(1);
    }

    // If it's a directory, import all .md files in it
    if expanded_path.is_dir() {
        let mut count = 0;
        for entry in std::fs::read_dir(&expanded_path)?.flatten() {
            let entry_path = entry.path();

            // Skip symlinks
            if entry_path
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true)
            {
                continue;
            }

            // Import .md files
            if entry_path.is_file() && entry_path.extension().map(|e| e == "md").unwrap_or(false) {
                import_single_agent(&entry_path)?;
                count += 1;
            }
        }

        if count == 0 {
            eprintln!("{} No .md files found in directory", "✘".red());
            std::process::exit(1);
        }

        return Ok(());
    }

    // Single file import
    import_single_agent(&expanded_path)
}

fn import_single_agent(source_path: &Path) -> Result<()> {
    // Derive agent name from path
    let agent_name = if source_path
        .file_name()
        .map(|n| n == "AGENT.md")
        .unwrap_or(false)
    {
        // Use parent directory name for AGENT.md files
        source_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "agent".to_string())
    } else {
        // Use filename without extension
        source_path
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "agent".to_string())
    };

    // Skip index.md
    if agent_name == "index" {
        return Ok(());
    }

    // Create target directory in global agents
    let global_agents_dir = home_dir()?.join(".config/barrel/agents");
    let target_dir = global_agents_dir.join(&agent_name);
    let target_file = target_dir.join("AGENT.md");

    if target_dir.exists() {
        // Silently skip existing agents when importing from directory
        println!(
            "{} {} {}/AGENT.md (already exists)",
            "-".dimmed(),
            "Skipped".dimmed(),
            agent_name
        );
        return Ok(());
    }

    std::fs::create_dir_all(&target_dir)?;
    std::fs::copy(source_path, &target_file)?;

    println!(
        "{} {} {}/AGENT.md",
        "✔".green(),
        "Imported".dimmed(),
        agent_name
    );

    Ok(())
}

fn rm_agent(name: &str, manifest_path: &Path, base_dir: &Path) -> Result<()> {
    use dialoguer::{Confirm, Select, theme::ColorfulTheme};

    let theme = ColorfulTheme::default();

    let local = AgentPath::local(name, base_dir);
    let global = AgentPath::global(name)?;

    let agent_to_remove = if local.exists() && global.exists() {
        let options = [
            format!("Local ({})", local.display()),
            format!("Global ({})", global.display()),
        ];
        let selection = Select::with_theme(&theme)
            .with_prompt(format!(
                "Agent '{}' exists in both locations. Which one to remove?",
                name
            ))
            .items(&options)
            .default(0)
            .interact()?;

        match selection {
            0 => local,
            1 => global,
            _ => unreachable!(),
        }
    } else if local.exists() {
        local
    } else if global.exists() {
        global
    } else {
        eprintln!("{}", format!("Agent '{}' not found", name).red());
        eprintln!();
        let _ = list_agents(manifest_path, base_dir);
        std::process::exit(1);
    };

    let confirmed = Confirm::with_theme(&theme)
        .with_prompt(format!("Remove {}?", agent_to_remove.display()))
        .default(false)
        .interact()?;

    if !confirmed {
        println!("{}", "Cancelled".dimmed());
        return Ok(());
    }

    std::fs::remove_dir_all(&agent_to_remove.dir)?;
    println!(
        "{} {} {}",
        "✔".green(),
        "Removed".dimmed(),
        agent_to_remove.display()
    );

    Ok(())
}
