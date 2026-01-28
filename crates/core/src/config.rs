//! Configuration types and parsing for axel workspaces
//!
//! This module provides the core configuration types for axel workspaces,
//! including workspace configuration, shell definitions, terminal profiles,
//! and skill management.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::Result;
use colored::Colorize;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// =============================================================================
// Workspace Configuration
// =============================================================================

/// Main workspace configuration loaded from AXEL.md (YAML frontmatter)
#[derive(Debug, Deserialize)]
pub struct WorkspaceConfig {
    /// Workspace name (used as tmux session name)
    #[serde(alias = "name")]
    pub workspace: String,
    /// Shell definitions
    #[serde(default)]
    pub shells: Vec<ShellConfig>,
    /// Terminal layout configuration
    pub terminal: TerminalConfig,
    /// Agent directories configuration
    #[serde(default)]
    pub skills: Vec<SkillPathConfig>,
    /// Path to the manifest file (set during loading, not from YAML)
    #[serde(skip)]
    pub manifest_path: Option<PathBuf>,
}

/// Configuration for an skill search path
#[derive(Debug, Deserialize, Clone)]
pub struct SkillPathConfig {
    /// Path to skills directory (relative to manifest or absolute)
    pub path: String,
}

impl WorkspaceConfig {
    /// Get all resolved skill directories that exist
    pub fn skills_dirs(&self) -> Vec<PathBuf> {
        let manifest_dir = self
            .manifest_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());

        self.skills
            .iter()
            .filter_map(|skill_config| {
                let path = &skill_config.path;
                let resolved = if path.starts_with('/') || path.starts_with('~') {
                    PathBuf::from(expand_path(path))
                } else if let Some(ref base) = manifest_dir {
                    base.join(path)
                } else {
                    PathBuf::from(path)
                };

                if resolved.exists() {
                    Some(resolved)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Find an skill file by name across all skill directories
    ///
    /// Supports both flat files (name.md) and directory structure (name/SKILL.md).
    /// Returns the first match (priority order defined by skills config).
    /// Warns if skill is found in multiple directories.
    pub fn find_skill(&self, name: &str) -> Option<PathBuf> {
        let dirs = self.skills_dirs();
        let mut first_match: Option<PathBuf> = None;
        let mut _first_dir: Option<PathBuf> = None;

        for dir in &dirs {
            // Check for directory structure: skills/<name>/SKILL.md
            let dir_path = dir.join(name).join("SKILL.md");
            if dir_path.exists() {
                if first_match.is_some() {
                    eprintln!(
                        "{} Duplicate skill '{}', ignoring {}",
                        "!".yellow(),
                        name,
                        dir.display()
                    );
                } else {
                    first_match = Some(dir_path);
                    _first_dir = Some(dir.clone());
                }
                continue;
            }

            // Check for flat file: skills/<name>.md (but not index.md)
            if name == "index" {
                continue;
            }
            let flat_path = dir.join(format!("{}.md", name));
            if flat_path.exists() {
                if first_match.is_some() {
                    eprintln!(
                        "{} Duplicate skill '{}', ignoring {}",
                        "!".yellow(),
                        name,
                        dir.display()
                    );
                } else {
                    first_match = Some(flat_path);
                    _first_dir = Some(dir.clone());
                }
            }
        }

        first_match
    }

    /// Find all skill files across all skill directories
    ///
    /// Uses priority order from config - first directory wins for conflicting names.
    /// Returns skills in priority order (preserves insertion order via IndexMap internally).
    pub fn find_all_skills(&self) -> Vec<PathBuf> {
        let mut skills_by_name: IndexMap<String, (PathBuf, PathBuf)> = IndexMap::new();

        for dir in self.skills_dirs() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();

                    let (skill_name, skill_path) = if path.is_dir() {
                        let skill_file = path.join("SKILL.md");
                        if skill_file.exists() {
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();
                            (name, skill_file)
                        } else {
                            continue;
                        }
                    } else if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                        // Skip index.md - it's used as workspace context, not an skill
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

                    if skill_name.is_empty() {
                        continue;
                    }

                    if let Some((existing_path, existing_dir)) = skills_by_name.get(&skill_name) {
                        eprintln!(
                            "{} Duplicate skill '{}', ignoring {}",
                            "!".yellow(),
                            skill_name,
                            dir.display()
                        );
                        let _ = (existing_path, existing_dir);
                    } else {
                        skills_by_name.insert(skill_name, (skill_path, dir.clone()));
                    }
                }
            }
        }

        skills_by_name.into_values().map(|(path, _)| path).collect()
    }

    /// Resolve skill paths based on config (supports "*" for all)
    pub fn resolve_skills(&self, skill_names: &[String]) -> Vec<PathBuf> {
        if skill_names.iter().any(|n| n == "*") {
            self.find_all_skills()
        } else {
            skill_names
                .iter()
                .filter_map(|name| self.find_skill(name))
                .collect()
        }
    }

    /// Load and parse skills from paths
    ///
    /// Returns skills in priority order (IndexMap preserves insertion order).
    #[allow(dead_code)]
    pub fn load_skills(&self, skill_names: &[String]) -> IndexMap<String, Skill> {
        let paths = self.resolve_skills(skill_names);
        let mut skills = IndexMap::new();

        for path in paths {
            if let Ok(skill) = Skill::from_file(&path) {
                skills.entry(skill.name.clone()).or_insert(skill);
            }
        }

        skills
    }

    /// Get workspace directory (parent of manifest)
    pub fn workspace_dir(&self) -> Option<PathBuf> {
        self.manifest_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
    }

    /// Load the workspace context from AXEL.md
    ///
    /// Reads the content after the YAML frontmatter from the manifest file.
    /// This content is used as initial context for AI assistants.
    pub fn load_index(&self) -> Option<WorkspaceIndex> {
        self.manifest_path
            .as_ref()
            .and_then(|path| WorkspaceIndex::from_manifest(path, &self.workspace).ok())
    }

    /// Get the profile type for a given profile name (defaults to "default")
    pub fn profile_type(&self, profile_name: Option<&str>) -> ProfileType {
        let profile_name = profile_name.unwrap_or("default");
        self.terminal
            .profiles
            .get(profile_name)
            .map(|p| p.profile_type)
            .unwrap_or_default()
    }

    /// Resolve panes using the specified profile (defaults to "default")
    pub fn resolve_panes(&self, profile_name: Option<&str>) -> Vec<ResolvedPane> {
        let profile_name = profile_name.unwrap_or("default");
        let Some(profile) = self.terminal.profiles.get(profile_name) else {
            return vec![];
        };

        // Build lookup map of shell templates by type
        let templates: HashMap<&str, &ShellConfig> =
            self.shells.iter().map(|s| (s.shell_type(), s)).collect();

        // Default path from manifest directory
        let default_path = self
            .workspace_dir()
            .map(|p| p.to_string_lossy().to_string());

        profile
            .panes
            .iter()
            .filter_map(|(pane_name, profile_pane)| {
                let shell_type = profile_pane
                    .shell_type
                    .as_deref()
                    .unwrap_or(pane_name.as_str());

                let template = templates.get(shell_type)?;

                let mut config = (*template).clone();

                if config.path().is_none()
                    && let Some(ref default) = default_path
                {
                    config.set_path(default.clone());
                }

                if let Some(ref color) = profile_pane.color {
                    config.set_color(color.clone());
                }

                Some(ResolvedPane {
                    name: pane_name.clone(),
                    col: profile_pane.col,
                    row: profile_pane.row,
                    width: profile_pane.width,
                    height: profile_pane.height,
                    config,
                })
            })
            .collect()
    }
}

// =============================================================================
// Skill Types
// =============================================================================

/// Parsed skill ready for AI tool configuration
#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    /// Agent name (derived from filename or frontmatter)
    #[serde(skip)]
    pub name: String,
    /// Description of when to use this skill
    pub description: String,
    /// The system prompt content
    pub prompt: String,
    /// Optional list of allowed tools
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Optional model to use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// YAML frontmatter for skill files
#[derive(Debug, Deserialize, Default)]
struct SkillFrontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tools: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

impl Skill {
    /// Parse an skill from a markdown file with optional YAML frontmatter
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;

        // Derive name from path
        let name = if path.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
            path.parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "skill".to_string())
        } else {
            path.file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "skill".to_string())
        };

        // Parse YAML frontmatter
        let (frontmatter, prompt) = if let Some(after_start) = content.strip_prefix("---") {
            if let Some(end_idx) = after_start.find("\n---") {
                let fm_content = &after_start[..end_idx];
                let rest = &after_start[end_idx + 4..];
                let fm: SkillFrontmatter = serde_yaml::from_str(fm_content).unwrap_or_default();
                (fm, rest.trim().to_string())
            } else {
                (SkillFrontmatter::default(), content)
            }
        } else {
            (SkillFrontmatter::default(), content)
        };

        let name = frontmatter.name.unwrap_or(name);

        let description = frontmatter.description.unwrap_or_else(|| {
            prompt
                .lines()
                .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .or_else(|| prompt.lines().next())
                .map(|l| l.trim_start_matches('#').trim().to_string())
                .unwrap_or_else(|| format!("{} skill", name))
        });

        let tools = frontmatter.tools.map(|t| {
            t.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        });

        Ok(Skill {
            name,
            description,
            prompt,
            tools,
            model: frontmatter.model,
        })
    }
}

// =============================================================================
// Workspace Index
// =============================================================================

/// Workspace index - project context from AXEL.md
///
/// This is NOT a skill - it's a project description used as initial context.
/// The content is extracted from AXEL.md after the YAML frontmatter.
#[derive(Debug, Clone)]
pub struct WorkspaceIndex {
    /// Project name (from workspace config)
    pub name: String,
    /// Project description from frontmatter
    pub description: Option<String>,
    /// Full markdown content (after frontmatter)
    pub content: String,
}

impl WorkspaceIndex {
    /// Parse a workspace index from the AXEL.md manifest file
    ///
    /// Extracts the content after the YAML frontmatter, which contains
    /// project documentation used as context for AI assistants.
    pub fn from_manifest(path: &Path, workspace_name: &str) -> Result<Self> {
        let raw_content = std::fs::read_to_string(path)?;

        // Extract content after YAML frontmatter
        let content = if let Some(after_start) = raw_content.strip_prefix("---") {
            if let Some(end_idx) = after_start.find("\n---") {
                after_start[end_idx + 4..].trim().to_string()
            } else {
                String::new()
            }
        } else {
            raw_content.trim().to_string()
        };

        // Return None-equivalent if no content after frontmatter
        if content.is_empty() {
            anyhow::bail!("No content after frontmatter in AXEL.md");
        }

        Ok(WorkspaceIndex {
            name: workspace_name.to_string(),
            description: None,
            content,
        })
    }

    /// Build the initial prompt to send to Claude/Codex
    pub fn to_initial_prompt(&self) -> String {
        format!(
            "Context: You're working on a project called {}. Here's the project documentation:\n\n{}\n\n---\nAwaiting your instructions.",
            self.name, self.content
        )
    }
}

// =============================================================================
// Terminal Configuration
// =============================================================================

/// Terminal layout configuration
#[derive(Debug, Deserialize)]
pub struct TerminalConfig {
    /// Named profiles with pane layouts
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
}

/// Terminal profile type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProfileType {
    /// Standard tmux session with panes
    #[default]
    Tmux,
    /// iTerm2 tmux control mode (-CC)
    TmuxCC,
    /// Direct shell execution (no tmux, first shell only)
    Shell,
}

impl<'de> serde::Deserialize<'de> for ProfileType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "tmux" => Ok(ProfileType::Tmux),
            "tmux_cc" => Ok(ProfileType::TmuxCC),
            "shell" => Ok(ProfileType::Shell),
            _ => Err(serde::de::Error::custom(format!(
                "unknown profile type: {} (expected tmux, tmux_cc, or shell)",
                s
            ))),
        }
    }
}

/// A terminal profile with type and pane definitions
#[derive(Debug, Clone)]
pub struct Profile {
    /// Profile type (tmux, tmux_cc, shell)
    pub profile_type: ProfileType,
    /// Pane definitions
    pub panes: IndexMap<String, ProfilePane>,
}

impl<'de> serde::Deserialize<'de> for Profile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut map: IndexMap<String, serde_yaml::Value> = IndexMap::deserialize(deserializer)?;

        let profile_type = if let Some(type_value) = map.shift_remove("type") {
            serde_yaml::from_value(type_value).map_err(serde::de::Error::custom)?
        } else {
            ProfileType::default()
        };

        let panes: IndexMap<String, ProfilePane> = map
            .into_iter()
            .filter_map(|(k, v)| serde_yaml::from_value(v).ok().map(|pane| (k, pane)))
            .collect();

        Ok(Profile {
            profile_type,
            panes,
        })
    }
}

/// Pane entry in a profile
#[derive(Debug, Deserialize, Default, Clone)]
pub struct ProfilePane {
    /// Reference to a shell type defined in shells
    pub shell_type: Option<String>,
    /// Column position
    #[serde(default)]
    pub col: u32,
    /// Row position
    #[serde(default)]
    pub row: u32,
    /// Width percentage
    #[serde(default)]
    pub width: Option<u32>,
    /// Height percentage
    #[serde(default)]
    pub height: Option<u32>,
    /// Override color from shell definition
    #[serde(default)]
    pub color: Option<String>,
}

// =============================================================================
// Shell Configuration
// =============================================================================

/// Raw shell config for deserialization
#[derive(Debug, Deserialize)]
struct ShellConfigRaw {
    #[serde(rename = "type")]
    shell_type: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    notes: Vec<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    disallowed_tools: Vec<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    command: Option<String>,
}

/// Shell configuration - known AI types or custom shell types
#[derive(Debug, Clone)]
pub enum ShellConfig {
    /// Claude Code shell
    Claude(AiShellConfig),
    /// Codex shell
    Codex(AiShellConfig),
    /// OpenCode shell
    Opencode(AiShellConfig),
    /// Google Antigravity shell
    Antigravity(AiShellConfig),
    /// Custom shell with arbitrary command
    Custom(CustomShellConfig),
}

impl<'de> serde::Deserialize<'de> for ShellConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = ShellConfigRaw::deserialize(deserializer)?;

        match raw.shell_type.as_str() {
            "claude" => Ok(ShellConfig::Claude(AiShellConfig {
                shell_type: raw.shell_type,
                path: raw.path,
                color: raw.color,
                notes: raw.notes,
                model: raw.model,
                skills: raw.skills,
                allowed_tools: raw.allowed_tools,
                disallowed_tools: raw.disallowed_tools,
                prompt: raw.prompt,
                args: raw.args,
            })),
            "codex" => Ok(ShellConfig::Codex(AiShellConfig {
                shell_type: raw.shell_type,
                path: raw.path,
                color: raw.color,
                notes: raw.notes,
                model: raw.model,
                skills: raw.skills,
                allowed_tools: raw.allowed_tools,
                disallowed_tools: raw.disallowed_tools,
                prompt: raw.prompt,
                args: raw.args,
            })),
            "opencode" => Ok(ShellConfig::Opencode(AiShellConfig {
                shell_type: raw.shell_type,
                path: raw.path,
                color: raw.color,
                notes: raw.notes,
                model: raw.model,
                skills: raw.skills,
                allowed_tools: raw.allowed_tools,
                disallowed_tools: raw.disallowed_tools,
                prompt: raw.prompt,
                args: raw.args,
            })),
            "antigravity" => Ok(ShellConfig::Antigravity(AiShellConfig {
                shell_type: raw.shell_type,
                path: raw.path,
                color: raw.color,
                notes: raw.notes,
                model: raw.model,
                skills: raw.skills,
                allowed_tools: raw.allowed_tools,
                disallowed_tools: raw.disallowed_tools,
                prompt: raw.prompt,
                args: raw.args,
            })),
            _ => Ok(ShellConfig::Custom(CustomShellConfig {
                shell_type: raw.shell_type,
                path: raw.path,
                color: raw.color,
                command: raw.command,
                notes: raw.notes,
            })),
        }
    }
}

impl ShellConfig {
    /// Get the shell type identifier
    pub fn shell_type(&self) -> &str {
        match self {
            ShellConfig::Claude(c)
            | ShellConfig::Codex(c)
            | ShellConfig::Opencode(c)
            | ShellConfig::Antigravity(c) => &c.shell_type,
            ShellConfig::Custom(c) => &c.shell_type,
        }
    }

    /// Get the color if set
    pub fn color(&self) -> Option<&str> {
        match self {
            ShellConfig::Claude(c)
            | ShellConfig::Codex(c)
            | ShellConfig::Opencode(c)
            | ShellConfig::Antigravity(c) => c.color.as_deref(),
            ShellConfig::Custom(c) => c.color.as_deref(),
        }
    }

    /// Set the color
    pub fn set_color(&mut self, color: String) {
        match self {
            ShellConfig::Claude(c)
            | ShellConfig::Codex(c)
            | ShellConfig::Opencode(c)
            | ShellConfig::Antigravity(c) => {
                c.color = Some(color);
            }
            ShellConfig::Custom(c) => c.color = Some(color),
        }
    }

    /// Get the path if set
    pub fn path(&self) -> Option<&str> {
        match self {
            ShellConfig::Claude(c)
            | ShellConfig::Codex(c)
            | ShellConfig::Opencode(c)
            | ShellConfig::Antigravity(c) => c.path.as_deref(),
            ShellConfig::Custom(c) => c.path.as_deref(),
        }
    }

    /// Set the path
    pub fn set_path(&mut self, path: String) {
        match self {
            ShellConfig::Claude(c)
            | ShellConfig::Codex(c)
            | ShellConfig::Opencode(c)
            | ShellConfig::Antigravity(c) => {
                c.path = Some(path);
            }
            ShellConfig::Custom(c) => c.path = Some(path),
        }
    }

    /// Get notes
    pub fn notes(&self) -> &[String] {
        match self {
            ShellConfig::Claude(c)
            | ShellConfig::Codex(c)
            | ShellConfig::Opencode(c)
            | ShellConfig::Antigravity(c) => &c.notes,
            ShellConfig::Custom(c) => &c.notes,
        }
    }
}

/// Configuration for AI shells (claude, codex, opencode)
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AiShellConfig {
    /// The shell type identifier
    #[serde(default, rename = "type")]
    pub shell_type: String,
    /// Working directory path
    #[serde(default)]
    pub path: Option<String>,
    /// Pane background color
    #[serde(default)]
    pub color: Option<String>,
    /// Notes to display in pane header
    #[serde(default)]
    pub notes: Vec<String>,
    /// Model to use (e.g., "sonnet", "opus")
    #[serde(default)]
    pub model: Option<String>,
    /// Agents to load - use "*" for all, or list specific names
    #[serde(default)]
    pub skills: Vec<String>,
    /// Allowed tools
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Disallowed tools
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    /// Initial prompt to send
    #[serde(default)]
    pub prompt: Option<String>,
    /// Additional CLI arguments
    #[serde(default)]
    pub args: Vec<String>,
}

/// Configuration for custom shell types
#[derive(Debug, Clone)]
pub struct CustomShellConfig {
    /// The type name
    pub shell_type: String,
    /// Working directory path
    pub path: Option<String>,
    /// Pane background color
    pub color: Option<String>,
    /// Command to execute
    pub command: Option<String>,
    /// Notes to display in pane header
    pub notes: Vec<String>,
}

impl Default for CustomShellConfig {
    fn default() -> Self {
        Self {
            shell_type: "shell".to_string(),
            path: None,
            color: None,
            command: None,
            notes: Vec::new(),
        }
    }
}

/// Resolved pane with config and layout merged
#[derive(Debug, Clone)]
pub struct ResolvedPane {
    /// Pane name
    pub name: String,
    /// Column position
    pub col: u32,
    /// Row position
    pub row: u32,
    /// Width percentage
    pub width: Option<u32>,
    /// Height percentage
    pub height: Option<u32>,
    /// Shell configuration
    pub config: ShellConfig,
}

impl ResolvedPane {
    /// Get the path if set
    pub fn path(&self) -> Option<&str> {
        self.config.path()
    }

    /// Get the color if set
    pub fn color(&self) -> Option<&str> {
        self.config.color()
    }

    /// Get notes
    pub fn notes(&self) -> &[String] {
        self.config.notes()
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Get the workspaces directory
pub fn workspaces_dir() -> PathBuf {
    PathBuf::from("/Users/ludovic/Coding/barrel/workspaces")
}

/// Extract YAML frontmatter from a markdown file.
/// Frontmatter is delimited by `---` at the start of the file.
fn extract_frontmatter(content: &str) -> Result<&str> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        anyhow::bail!("No frontmatter found: file must start with ---");
    }
    let after_opening = &trimmed[3..];
    let after_opening = after_opening.strip_prefix('\n').unwrap_or(after_opening);
    match after_opening.find("\n---") {
        Some(end) => Ok(&after_opening[..end]),
        None => anyhow::bail!("No closing --- found for frontmatter"),
    }
}

/// Load workspace configuration from a file.
/// Parses YAML from markdown frontmatter.
pub fn load_config(path: &Path) -> Result<WorkspaceConfig> {
    let content = std::fs::read_to_string(path)?;
    let yaml = extract_frontmatter(&content)?;
    let mut config: WorkspaceConfig = serde_yaml::from_str(yaml)?;
    config.manifest_path = Some(path.to_path_buf());
    Ok(config)
}

/// Generate a new workspace configuration as a markdown file with YAML frontmatter
pub fn generate_config(workspace: &str, _workspace_path: &str) -> String {
    format!(
        r#"---
workspace: {workspace}

# =============================================================================
# Skill directories
# =============================================================================
# Search paths for skill files (first match wins for duplicate names)
# Supports: ./relative, ~/home, /absolute paths

skills:
  - path: ./skills
  - path: ~/.config/axel/skills

# =============================================================================
# Shell definitions
# =============================================================================
# Define shells that can be used in terminal profiles
#
# Built-in types: claude, codex, opencode, antigravity, shell
# Custom types use the 'command' field

shells:
  # Claude Code - AI coding assistant
  - type: claude
    color: gray
    skills:
      - "*"                    # Load all skills, or list specific: ["skill1", "skill2"]
    # model: sonnet            # Model: sonnet, opus, haiku
    # prompt: "Your task..."   # Initial prompt
    # allowed_tools: []        # Restrict to specific tools
    # disallowed_tools: []     # Block specific tools
    # args: []                 # Additional CLI arguments

  # Codex - OpenAI coding assistant
  # - type: codex
  #   color: green
  #   skills: ["*"]
  #   # model: gpt-4           # Model to use

  # OpenCode - Open-source coding assistant
  # - type: opencode
  #   color: blue
  #   skills: ["*"]

  # Antigravity - Google coding assistant
  # - type: antigravity
  #   color: orange
  #   skills: ["*"]
  #   # model: gemini-3-pro    # Model to use

  # Regular shell with notes displayed on startup
  - type: shell
    notes:
      - "$ axel -k {workspace}"

  # Custom command example
  # - type: logs
  #   command: "tail -f /var/log/app.log"
  #   color: red

# =============================================================================
# Terminal profiles
# =============================================================================
# Layout configurations for tmux sessions
#
# Profile types:
#   tmux    - Standard tmux session (default)
#   tmux_cc - iTerm2 tmux integration mode
#   shell   - No tmux, run first pane directly
#
# Pane positioning:
#   col: 0, 1, 2...  - Column position (left to right)
#   row: 0, 1, 2...  - Row position within column (top to bottom)
#   width: 50        - Column width percentage
#   height: 30       - Row height percentage
#
# Colors: purple, yellow, red, green, blue, gray, orange

terminal:
  profiles:
    # Default profile - two columns
    default:
      type: tmux
      claude:
        col: 0
        row: 0
      shell:
        col: 1
        row: 0
        color: yellow

    # Solo mode - single AI pane
    # solo:
    #   type: shell
    #   claude:
    #     col: 0
    #     row: 0

    # Three column layout
    # wide:
    #   type: tmux
    #   claude:
    #     col: 0
    #     row: 0
    #     width: 40
    #   codex:
    #     col: 1
    #     row: 0
    #     width: 40
    #   shell:
    #     col: 2
    #     row: 0
    #     width: 20
---

# {workspace}

<!-- Project context for AI assistants. This content is used as initial context when launching shells. -->

## Overview

<!-- Brief description of what this project does -->

## Getting Started

<!-- How to set up and run the project -->

## Architecture

<!-- High-level architecture overview -->

## Key Files

<!-- Important files and directories -->
"#,
        workspace = workspace,
    )
}

/// Convert color name to tmux color code
pub fn to_tmux_color(color: &str) -> &'static str {
    match color {
        "purple" => "#251F2B",
        "yellow" => "#2B2011",
        "red" => "#231517",
        "green" => "#122322",
        "blue" => "#1E202E",
        "gray" | "grey" => "#1a1a1a",
        "orange" => "#2B2011",
        _ => "default",
    }
}

/// Convert color name to RGB for terminal escape sequences
pub fn to_fg_rgb(color: &str) -> &'static str {
    match color {
        "purple" => "198;147;241",
        "yellow" => "255;182;21",
        "red" => "251;109;136",
        "green" => "0;217;146",
        "blue" => "133;162;255",
        "gray" | "grey" => "150;150;150",
        "orange" => "255;182;21",
        _ => "255;255;255",
    }
}

/// Expand ~ to home directory in paths
pub fn expand_path(path: &str) -> String {
    path.strip_prefix("~/")
        .and_then(|stripped| dirs::home_dir().map(|home| home.join(stripped)))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_parsing_without_frontmatter() {
        let content = "# Test Agent\n\nYou are a helpful skill.";
        let temp_dir = std::env::temp_dir();
        let skill_path = temp_dir.join("test-skill.md");
        std::fs::write(&skill_path, content).unwrap();

        let skill = Skill::from_file(&skill_path).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.prompt, content);
        assert!(skill.description.contains("Test Agent") || skill.description.contains("helpful"));

        std::fs::remove_file(&skill_path).ok();
    }

    #[test]
    fn test_skill_parsing_with_frontmatter() {
        let content = r#"---
name: custom-name
description: A custom description
tools: Read, Write, Bash
model: opus
---

# My Agent

You are a specialized skill."#;

        let temp_dir = std::env::temp_dir();
        let skill_path = temp_dir.join("frontmatter-skill.md");
        std::fs::write(&skill_path, content).unwrap();

        let skill = Skill::from_file(&skill_path).unwrap();
        assert_eq!(skill.name, "custom-name");
        assert_eq!(skill.description, "A custom description");
        assert_eq!(
            skill.tools,
            Some(vec![
                "Read".to_string(),
                "Write".to_string(),
                "Bash".to_string()
            ])
        );
        assert_eq!(skill.model, Some("opus".to_string()));
        assert!(skill.prompt.contains("My Agent"));
        assert!(skill.prompt.contains("specialized skill"));

        std::fs::remove_file(&skill_path).ok();
    }

    #[test]
    fn test_skill_dir_structure() {
        let temp_dir = std::env::temp_dir().join("axel-test-skills");
        let skill_dir = temp_dir.join("my-skill");
        std::fs::create_dir_all(&skill_dir).ok();

        let skill_file = skill_dir.join("SKILL.md");
        std::fs::write(&skill_file, "# My Agent\n\nHello").unwrap();

        let skill = Skill::from_file(&skill_file).unwrap();
        assert_eq!(skill.name, "my-skill");

        std::fs::remove_dir_all(&temp_dir).ok();
    }
}
