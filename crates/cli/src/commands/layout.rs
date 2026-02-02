//! Layout listing commands for axel.
//!
//! Provides commands to query layout configurations from AXEL.md manifests.

use std::path::Path;

use anyhow::Result;
use axel_core::config::{PaneConfig, load_config};
use serde::Serialize;

/// JSON output format for a pane configuration
#[derive(Serialize)]
pub struct PaneInfo {
    /// Pane type identifier (e.g., "claude", "codex", "shell")
    #[serde(rename = "type")]
    pub pane_type: String,
    /// Display name for the pane
    pub name: String,
    /// Color associated with this pane
    pub color: Option<String>,
    /// Whether this is an AI pane (vs custom command)
    pub is_ai: bool,
}

impl From<&PaneConfig> for PaneInfo {
    fn from(config: &PaneConfig) -> Self {
        let (pane_type, color, is_ai) = match config {
            PaneConfig::Claude(c) => (c.pane_type.clone(), c.color.clone(), true),
            PaneConfig::Codex(c) => (c.pane_type.clone(), c.color.clone(), true),
            PaneConfig::Opencode(c) => (c.pane_type.clone(), c.color.clone(), true),
            PaneConfig::Antigravity(c) => (c.pane_type.clone(), c.color.clone(), true),
            PaneConfig::Custom(c) => (c.pane_type.clone(), c.color.clone(), false),
        };

        // Generate display name from type
        let name = match pane_type.as_str() {
            "claude" => "Claude".to_string(),
            "codex" => "Codex".to_string(),
            "opencode" => "OpenCode".to_string(),
            "antigravity" => "Antigravity".to_string(),
            "shell" => "Shell".to_string(),
            other => {
                // Capitalize first letter for custom types
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => other.to_string(),
                }
            }
        };

        PaneInfo {
            pane_type,
            name,
            color,
            is_ai,
        }
    }
}

/// List all panes defined in the workspace AXEL.md
pub fn list_panes(manifest_path: Option<&str>, _json: bool) -> Result<()> {
    let path = manifest_path.unwrap_or("./AXEL.md");
    let config = load_config(Path::new(path))?;

    let panes: Vec<PaneInfo> = config.layouts.panes.iter().map(PaneInfo::from).collect();

    // Always output JSON for now (the flag is for future plain text support)
    let json = serde_json::to_string_pretty(&panes)?;
    println!("{}", json);

    Ok(())
}
