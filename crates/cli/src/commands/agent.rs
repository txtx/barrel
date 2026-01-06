//! Agent management commands for barrel.
//!
//! This module handles all agent-related operations:
//! - Listing agents (local and global)
//! - Creating new agents
//! - Importing agents from files/directories
//! - Forking global agents to local
//! - Linking global agents to local
//! - Removing agents

use std::path::{Path, PathBuf};

use anyhow::Result;
use barrel_core::{config::load_config, drivers};
use colored::Colorize;

use crate::{display_path, home_dir};

// =============================================================================
// Constants
// =============================================================================

const AGENT_FILE: &str = "AGENT.md";
const AGENTS_DIR: &str = "agents";
const BARREL_DIR: &str = "barrel";
const CONFIG_DIR: &str = ".config";

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

// =============================================================================
// Public Commands
// =============================================================================

/// Clean up installed agent symlinks for all drivers
pub fn cleanup_agents(workspace_dir: &Path) -> Vec<&'static str> {
    let mut cleaned = Vec::new();

    for driver in drivers::all_drivers() {
        if driver.cleanup(workspace_dir) {
            cleaned.push(driver.name());
        }
    }

    cleaned
}

/// Format cleaned drivers list for display
pub fn format_cleaned_drivers(cleaned: &[&str]) -> String {
    if cleaned.len() == 1 {
        cleaned[0].to_string()
    } else {
        let last = cleaned.last().unwrap();
        let rest = &cleaned[..cleaned.len() - 1];
        format!("{} and {}", rest.join(", "), last)
    }
}

/// List all available agents (local and global)
pub fn list_agents(manifest_path: &Path, base_dir: &Path) -> Result<()> {
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

/// Create a new agent interactively
pub fn new_agent(name: Option<&str>, base_dir: &Path) -> Result<()> {
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

/// Import agent file(s) to the global agents directory
pub fn import_agent(path: &str) -> Result<()> {
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

/// Fork (copy) a global agent to the current workspace
pub fn fork_agent(name: &str, manifest_path: &Path, base_dir: &Path) -> Result<()> {
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

/// Link (symlink) a global agent to the current workspace
pub fn link_agent(name: &str, manifest_path: &Path, base_dir: &Path) -> Result<()> {
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

/// Remove an agent
pub fn rm_agent(name: &str, manifest_path: &Path, base_dir: &Path) -> Result<()> {
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
