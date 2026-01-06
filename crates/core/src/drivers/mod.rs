//! Agent driver implementations
//!
//! This module provides the `AgentDriver` trait and implementations for various
//! AI coding tools (Claude Code, Codex, OpenCode, Antigravity). Drivers handle
//! installing agent files in tool-specific formats.

mod antigravity;
mod claude;
mod codex;
mod opencode;

use std::path::{Path, PathBuf};

pub use antigravity::AntigravityDriver;
use anyhow::Result;
pub use claude::ClaudeDriver;
pub use codex::CodexDriver;
pub use opencode::OpenCodeDriver;

/// Trait for agent installation drivers
///
/// Each driver knows how to install agents for a specific tool (Claude Code, Codex, etc.)
pub trait AgentDriver {
    /// Driver name for display/config
    fn name(&self) -> &'static str;

    /// Directory where agents should be installed (relative to workspace)
    fn agents_dir(&self, workspace_dir: &Path) -> PathBuf;

    /// File patterns used by this driver for agent discovery
    ///
    /// Returns patterns like "CLAUDE.md", ".claude/agents/*.md", etc.
    fn agent_patterns(&self) -> &'static [&'static str];

    /// Install agents to the target directory
    ///
    /// Returns the number of agents installed.
    fn install_agents(&self, workspace_dir: &Path, agent_paths: &[PathBuf]) -> Result<usize>;

    /// Clean up installed agents from the workspace
    ///
    /// Returns true if any cleanup was performed.
    fn cleanup(&self, workspace_dir: &Path) -> bool;
}

/// Get a driver by name
pub fn get_driver(name: &str) -> Option<Box<dyn AgentDriver>> {
    match name {
        "claude" => Some(Box::new(ClaudeDriver)),
        "codex" => Some(Box::new(CodexDriver)),
        "opencode" => Some(Box::new(OpenCodeDriver)),
        "antigravity" => Some(Box::new(AntigravityDriver)),
        _ => None,
    }
}

/// Get all available drivers
pub fn all_drivers() -> Vec<Box<dyn AgentDriver>> {
    vec![
        Box::new(ClaudeDriver),
        Box::new(CodexDriver),
        Box::new(OpenCodeDriver),
        Box::new(AntigravityDriver),
    ]
}

/// Get all agent file patterns from all drivers
pub fn all_agent_patterns() -> Vec<&'static str> {
    let mut patterns = Vec::new();
    // Also include generic agent patterns
    patterns.extend_from_slice(&["agents/*.md", "agents/*/AGENT.md"]);
    for driver in all_drivers() {
        patterns.extend_from_slice(driver.agent_patterns());
    }
    patterns
}
