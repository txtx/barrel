//! Codex skill driver.
//!
//! Codex expects skills in `.codex/skills/<skill-name>/SKILL.md` format.
//! This driver creates skill directories with symlinked SKILL.md files,
//! making them discoverable via the `/skills` slash command.
//!
//! Codex also uses AGENTS.md for project context (similar to Claude's CLAUDE.md).
//!
//! ## OpenTelemetry Support
//!
//! Codex supports OTEL telemetry export via standard environment variables.
//! See: https://developers.openai.com/codex/config-advanced/

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::claude::install_index_symlink;
use super::SkillDriver;
use crate::config::WorkspaceConfig;
use crate::hooks::{otel_logs_endpoint, otel_metrics_endpoint, otel_traces_endpoint};

/// Codex skill driver
pub struct CodexDriver;

impl SkillDriver for CodexDriver {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn skills_dir(&self, workspace_dir: &Path) -> PathBuf {
        workspace_dir.join(".codex").join("skills")
    }

    fn skill_patterns(&self) -> &'static [&'static str] {
        &["AGENTS.md", ".codex/skills/*/SKILL.md"]
    }

    fn install_skills(&self, workspace_dir: &Path, skill_paths: &[PathBuf]) -> Result<usize> {
        if skill_paths.is_empty() {
            return Ok(0);
        }

        let skills_dir = self.skills_dir(workspace_dir);
        std::fs::create_dir_all(&skills_dir)?;

        let mut count = 0;
        for source_path in skill_paths {
            let name = derive_skill_name(source_path);

            // Codex expects: .codex/skills/<skill-name>/SKILL.md
            let skill_dir = skills_dir.join(&name);
            let link_path = skill_dir.join("SKILL.md");

            // Remove existing skill directory if present
            if skill_dir.exists() {
                std::fs::remove_dir_all(&skill_dir).ok();
            }

            // Create skill directory
            std::fs::create_dir_all(&skill_dir)?;

            // Canonicalize the source path to get a clean absolute path
            let canonical_source = source_path
                .canonicalize()
                .unwrap_or_else(|_| source_path.clone());

            // Create symlink to SKILL.md
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

        // Remove skill directories from .codex/skills/
        let skills_dir = self.skills_dir(workspace_dir);
        if skills_dir.exists()
            && let Ok(entries) = std::fs::read_dir(&skills_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Check if this directory contains a SKILL.md symlink
                    let skill_md = path.join("SKILL.md");
                    if skill_md
                        .symlink_metadata()
                        .map(|m| m.file_type().is_symlink())
                        .unwrap_or(false)
                        && std::fs::remove_dir_all(&path).is_ok()
                    {
                        cleaned = true;
                    }
                }
            }
        }

        // Remove AGENTS.md symlink
        let agents_md = workspace_dir.join("AGENTS.md");
        if agents_md
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
            && std::fs::remove_file(&agents_md).is_ok()
        {
            cleaned = true;
        }

        cleaned
    }

    fn supports_otel(&self) -> bool {
        true
    }

    fn otel_env_vars(&self, port: u16, pane_id: &str) -> Vec<(String, String)> {
        // Codex uses standard OTEL environment variables for telemetry export.
        // Unlike Claude, Codex primarily exports logs (not metrics).
        // See: https://developers.openai.com/codex/config-advanced/
        vec![
            // Use HTTP JSON protocol (our server accepts this)
            (
                "OTEL_EXPORTER_OTLP_PROTOCOL".to_string(),
                "http/json".to_string(),
            ),
            // Set specific endpoints for each signal type
            (
                "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT".to_string(),
                otel_logs_endpoint(port, pane_id),
            ),
            (
                "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT".to_string(),
                otel_metrics_endpoint(port, pane_id),
            ),
            (
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT".to_string(),
                otel_traces_endpoint(port, pane_id),
            ),
            // Enable log exporter
            ("OTEL_LOGS_EXPORTER".to_string(), "otlp".to_string()),
        ]
    }

    fn index_filename(&self) -> Option<&'static str> {
        Some("AGENTS.md")
    }

    fn install_index(&self, config: &WorkspaceConfig, workspace_dir: &Path) -> Result<bool> {
        install_index_symlink(config, workspace_dir, "AGENTS.md")
    }
}

/// Derive skill name from file path.
///
/// Handles two naming conventions:
/// - `<name>/SKILL.md` -> uses the directory name
/// - `<name>.md` -> uses the file stem
fn derive_skill_name(path: &Path) -> String {
    if path.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
        path.parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "skill".to_string())
    } else {
        path.file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "skill".to_string())
    }
}
