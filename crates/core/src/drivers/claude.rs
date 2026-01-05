//! Claude Code agent driver
//!
//! Installs agents to `.claude/agents/` directory as symlinks and creates
//! CLAUDE.md symlink pointing to agents/index.md if present.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::AgentDriver;
use crate::config::WorkspaceConfig;

/// Claude Code agent driver
pub struct ClaudeDriver;

impl AgentDriver for ClaudeDriver {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn agents_dir(&self, workspace_dir: &Path) -> PathBuf {
        workspace_dir.join(".claude").join("agents")
    }

    fn agent_patterns(&self) -> &'static [&'static str] {
        &["CLAUDE.md", ".claude/agents/*.md"]
    }

    fn install_agents(&self, workspace_dir: &Path, agent_paths: &[PathBuf]) -> Result<usize> {
        if agent_paths.is_empty() {
            return Ok(0);
        }

        let agents_dir = self.agents_dir(workspace_dir);
        std::fs::create_dir_all(&agents_dir)?;

        let mut count = 0;
        for source_path in agent_paths {
            let name = derive_agent_name(source_path);
            let link_path = agents_dir.join(format!("{}.md", name));

            // Remove existing symlink/file if present
            if link_path.exists() || link_path.is_symlink() {
                std::fs::remove_file(&link_path).ok();
            }

            // Canonicalize the source path to get a clean absolute path
            let canonical_source = source_path
                .canonicalize()
                .unwrap_or_else(|_| source_path.clone());

            // Create symlink
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&canonical_source, &link_path)?;
                count += 1;
            }
        }

        Ok(count)
    }

    fn cleanup(&self, workspace_dir: &Path) -> bool {
        let mut cleaned = false;

        // Remove agent symlinks from .claude/agents/
        let agents_dir = self.agents_dir(workspace_dir);
        if agents_dir.exists()
            && let Ok(entries) = std::fs::read_dir(&agents_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .symlink_metadata()
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                    && std::fs::remove_file(&path).is_ok()
                {
                    cleaned = true;
                }
            }
        }

        // Remove CLAUDE.md symlink
        let claude_md = workspace_dir.join("CLAUDE.md");
        if claude_md
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
            && std::fs::remove_file(&claude_md).is_ok()
        {
            cleaned = true;
        }

        cleaned
    }
}

impl ClaudeDriver {
    /// Create CLAUDE.md symlink pointing to agents/index.md if it exists
    pub fn install_index(&self, config: &WorkspaceConfig, workspace_dir: &Path) -> Result<bool> {
        // Find index.md in agent directories
        for dir in config.agents_dirs() {
            let index_path = dir.join("index.md");
            if index_path.exists() {
                let link_path = workspace_dir.join("CLAUDE.md");

                // Remove existing symlink/file if present
                if link_path.exists() || link_path.is_symlink() {
                    std::fs::remove_file(&link_path).ok();
                }

                // Canonicalize the source path
                let canonical_source = index_path
                    .canonicalize()
                    .unwrap_or_else(|_| index_path.clone());

                // Create symlink
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(&canonical_source, &link_path)?;
                    return Ok(true);
                }

                #[cfg(not(unix))]
                return Ok(false);
            }
        }
        Ok(false)
    }
}

/// Derive agent name from file path
///
/// - For AGENT.md files, use parent directory name
/// - For other .md files, use filename without extension
fn derive_agent_name(path: &Path) -> String {
    if path.file_name().map(|n| n == "AGENT.md").unwrap_or(false) {
        path.parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "agent".to_string())
    } else {
        path.file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "agent".to_string())
    }
}
