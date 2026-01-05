//! OpenCode agent driver.
//!
//! OpenCode uses a similar symlink strategy to Claude Code. Agents are installed
//! as symlinks in `.opencode/agent/` directory within the workspace.
//!
//! This driver:
//! 1. Creates `.opencode/agent/` if it doesn't exist
//! 2. Symlinks each agent file as `<name>.md`
//! 3. On cleanup, removes only symlinks (preserving any manually created files)

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::AgentDriver;

/// OpenCode agent driver
pub struct OpenCodeDriver;

impl AgentDriver for OpenCodeDriver {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn agents_dir(&self, workspace_dir: &Path) -> PathBuf {
        workspace_dir.join(".opencode").join("agent")
    }

    fn agent_patterns(&self) -> &'static [&'static str] {
        &[".opencode/agent/*.md", ".opencode/AGENT.md"]
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
        let agents_dir = self.agents_dir(workspace_dir);
        if !agents_dir.exists() {
            return false;
        }

        let mut cleaned = false;
        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
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

        cleaned
    }
}

/// Derive agent name from file path.
///
/// Handles two naming conventions:
/// - `<name>/AGENT.md` → uses the directory name
/// - `<name>.md` → uses the file stem
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
