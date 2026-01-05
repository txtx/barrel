//! Tmux workspace session management.
//!
//! This module provides high-level workspace creation using tmux sessions.
//! It handles the complex layout algorithm for arranging panes in a grid,
//! installing agents for each AI tool, and configuring tmux with barrel styling.
//!
//! # Layout Algorithm
//!
//! Panes are organized in a column-major grid:
//! 1. Panes are sorted by (col, row)
//! 2. Columns are created via horizontal splits from left to right
//! 3. Rows within each column are created via vertical splits
//! 4. Width/height percentages are applied during splits
//!
//! # Session Features
//!
//! - Mouse support with clipboard integration
//! - Pane border titles showing shell names
//! - Color-coded panes based on shell configuration
//! - Automatic agent installation per driver type
//! - Manifest path stored in session environment for cleanup

use std::{collections::HashMap, io::Write};

use anyhow::Result;
use colored::Colorize;

use super::commands::{
    NewSession, SelectPane, SetOption, SplitWindow, bind_key, get_pane_id, rename_window,
    send_keys, set_environment,
};
use crate::{
    claude::ClaudeCommand,
    config::{
        AiShellConfig, ResolvedPane, ShellConfig, WorkspaceConfig, WorkspaceIndex, expand_path,
        to_fg_rgb, to_tmux_color,
    },
    drivers,
};

// =============================================================================
// Tmux option keys
// =============================================================================

const OPT_MOUSE: &str = "mouse";
const OPT_SET_CLIPBOARD: &str = "set-clipboard";
const OPT_ALLOW_PASSTHROUGH: &str = "allow-passthrough";
const OPT_EXTENDED_KEYS: &str = "extended-keys";
const OPT_PANE_BORDER_STATUS: &str = "pane-border-status";
const OPT_PANE_BORDER_FORMAT: &str = "pane-border-format";
const OPT_PANE_ACTIVE_BORDER_STYLE: &str = "pane-active-border-style";
const OPT_STATUS_STYLE: &str = "status-style";
const OPT_STATUS_RIGHT: &str = "status-right";
const OPT_ALLOW_RENAME: &str = "allow-rename";

// =============================================================================
// Tmux option values
// =============================================================================

const VAL_ON: &str = "on";
const VAL_OFF: &str = "off";
const VAL_TOP: &str = "top";

// =============================================================================
// Tmux key bindings
// =============================================================================

const KEY_TABLE_COPY_MODE: &str = "copy-mode";
const KEY_MOUSE_DRAG_END: &str = "MouseDragEnd1Pane";
const KEY_WHEEL_UP: &str = "WheelUpPane";
const KEY_WHEEL_DOWN: &str = "WheelDownPane";

// =============================================================================
// Barrel-specific constants
// =============================================================================

/// Barrel accent color (blue)
const BARREL_COLOR: &str = "#85A2FF";
/// Pane border format template
const PANE_BORDER_FORMAT: &str = "#[align=centre] #{pane_title} ";

/// Environment variable name for storing manifest path in tmux session
pub const BARREL_MANIFEST_ENV: &str = "BARREL_MANIFEST";

/// Build the command string for an AI shell (Claude or OpenCode).
///
/// Both Claude Code and OpenCode use similar CLI interfaces, so this function
/// handles both by parameterizing the command name. The command is built using
/// `ClaudeCommand` builder which handles argument escaping and formatting.
///
/// Note: The `_index` parameter is unused because index content is handled via
/// CLAUDE.md symlink for Claude (installed by the driver).
fn build_ai_command(
    command_name: &str,
    config: &AiShellConfig,
    _index: Option<&WorkspaceIndex>,
) -> String {
    let mut cmd = ClaudeCommand::new();

    if let Some(model) = &config.model {
        cmd = cmd.model(model);
    }
    if !config.allowed_tools.is_empty() {
        cmd = cmd.allowed_tools(config.allowed_tools.clone());
    }
    if !config.disallowed_tools.is_empty() {
        cmd = cmd.disallowed_tools(config.disallowed_tools.clone());
    }
    // Only use explicit prompt - index is handled via CLAUDE.md symlink for Claude
    if let Some(prompt) = &config.prompt {
        cmd = cmd.prompt(prompt);
    }
    for arg in &config.args {
        cmd = cmd.extra_arg(arg);
    }

    let built = cmd.build();
    // Replace "claude" with actual command if different
    if command_name != "claude" {
        built.replacen("claude", command_name, 1)
    } else {
        built
    }
}

/// Build the command string for Codex CLI.
///
/// Codex has a different CLI interface than Claude/OpenCode. Key differences:
/// - Uses `-c` for config options instead of dedicated flags
/// - Agents are discovered via `project_doc_fallback_filenames` config
/// - Initial prompt is passed as a positional argument
///
/// The command includes `-c 'project_doc_fallback_filenames=[".codex/AGENTS.md"]'`
/// to ensure Codex discovers the merged agents file created by the driver.
fn build_codex_command(
    config: &AiShellConfig,
    _workspace_dir: Option<&std::path::Path>,
    index: Option<&WorkspaceIndex>,
) -> String {
    let mut parts = vec!["codex".to_string()];

    // Add .codex/AGENTS.md to fallback filenames so Codex discovers it
    parts.push("-c".to_string());
    parts.push("'project_doc_fallback_filenames=[\".codex/AGENTS.md\"]'".to_string());

    if let Some(model) = &config.model {
        parts.push("-m".to_string());
        parts.push(model.clone());
    }

    for arg in &config.args {
        parts.push(arg.clone());
    }

    // Use single quotes for shell safety
    if let Some(prompt) = &config.prompt {
        let escaped = prompt.replace('\'', "'\\''");
        parts.push(format!("'{}'", escaped));
    } else if let Some(idx) = index {
        let escaped = idx.to_initial_prompt().replace('\'', "'\\''");
        parts.push(format!("'{}'", escaped));
    }

    parts.join(" ")
}

/// Build the command to run for a pane
pub fn build_pane_command(
    pane: &ResolvedPane,
    workspace_dir: Option<&std::path::Path>,
    index: Option<&WorkspaceIndex>,
) -> Option<String> {
    match &pane.config {
        ShellConfig::Claude(config) => Some(build_ai_command("claude", config, index)),
        ShellConfig::Codex(config) => Some(build_codex_command(config, workspace_dir, index)),
        ShellConfig::Opencode(config) => Some(build_ai_command("opencode", config, index)),
        ShellConfig::Custom(config) => config.command.clone(),
    }
}

/// Create a tmux workspace from a configuration.
///
/// This is the main entry point for workspace creation. It:
///
/// 1. **Resolves panes** from the profile configuration
/// 2. **Installs agents** for each AI driver (Claude, Codex, OpenCode)
/// 3. **Creates the tmux session** with the first pane
/// 4. **Configures session options** (mouse, clipboard, styling)
/// 5. **Builds the grid layout** via horizontal/vertical splits
/// 6. **Sends commands** to each pane to launch the shells
///
/// The layout algorithm groups panes by column, creates columns via horizontal
/// splits, then creates rows within each column via vertical splits. Width/height
/// percentages are applied during the split operations.
pub fn create_workspace(
    session_name: &str,
    config: &WorkspaceConfig,
    profile: Option<&str>,
) -> Result<()> {
    let mut panes = config.resolve_panes(profile);
    let workspace_dir = config.workspace_dir();
    let index = config.load_index();

    if panes.is_empty() {
        anyhow::bail!("No panes defined");
    }

    // Collect agent names per driver type from AI panes
    let mut claude_agents: Vec<String> = Vec::new();
    let mut codex_agents: Vec<String> = Vec::new();
    let mut opencode_agents: Vec<String> = Vec::new();

    for pane in &panes {
        match &pane.config {
            ShellConfig::Claude(c) => claude_agents.extend(c.agents.iter().cloned()),
            ShellConfig::Codex(c) => codex_agents.extend(c.agents.iter().cloned()),
            ShellConfig::Opencode(c) => opencode_agents.extend(c.agents.iter().cloned()),
            ShellConfig::Custom(_) => {}
        }
    }
    claude_agents.dedup();
    codex_agents.dedup();
    opencode_agents.dedup();

    // Install agents for each driver that has panes
    if let Some(ref workspace_dir) = workspace_dir {
        for (driver_name, agent_names) in [
            ("claude", &claude_agents),
            ("codex", &codex_agents),
            ("opencode", &opencode_agents),
        ] {
            if agent_names.is_empty() {
                continue;
            }
            let Some(driver) = drivers::get_driver(driver_name) else {
                continue;
            };
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

        // Install CLAUDE.md symlink pointing to agents/index.md if present
        let has_claude_panes = panes
            .iter()
            .any(|p| matches!(p.config, ShellConfig::Claude(_)));
        if has_claude_panes {
            let claude_driver = drivers::ClaudeDriver;
            if claude_driver
                .install_index(config, workspace_dir)
                .unwrap_or(false)
            {
                eprintln!("{} {} CLAUDE.md symlink", "✔".green(), "Created".dimmed());
            }
        }
    }

    // Sort panes by col, then row
    panes.sort_by(|a, b| a.col.cmp(&b.col).then(a.row.cmp(&b.row)));

    // Group panes by column
    let mut columns: HashMap<u32, Vec<&ResolvedPane>> = HashMap::new();
    let mut col_widths: HashMap<u32, u32> = HashMap::new();
    let mut max_col = 0;

    for pane in &panes {
        columns.entry(pane.col).or_default().push(pane);
        if let Some(width) = pane.width {
            col_widths.insert(pane.col, width);
        }
        if pane.col > max_col {
            max_col = pane.col;
        }
    }

    // Create session with first pane
    let first_pane = &panes[0];
    let first_path = first_pane
        .path()
        .map(expand_path)
        .unwrap_or_else(|| ".".to_string());

    NewSession::new()
        .name(session_name)
        .detached()
        .start_directory(&first_path)
        .run()?;

    // Store manifest path in session environment for cleanup on kill
    if let Some(manifest_path) = &config.manifest_path
        && let Some(path_str) = manifest_path.to_str()
    {
        set_environment(session_name, BARREL_MANIFEST_ENV, path_str).ok();
    }

    // Configure session options
    SetOption::new()
        .target(session_name)
        .option(OPT_MOUSE)
        .value(VAL_ON)
        .run()?;

    SetOption::new()
        .target(session_name)
        .option(OPT_SET_CLIPBOARD)
        .value(VAL_ON)
        .run()?;

    SetOption::new()
        .global()
        .option(OPT_ALLOW_PASSTHROUGH)
        .value(VAL_ON)
        .run()
        .ok();

    SetOption::new()
        .target(session_name)
        .option(OPT_EXTENDED_KEYS)
        .value(VAL_ON)
        .run()
        .ok();

    SetOption::new()
        .target(session_name)
        .option(OPT_PANE_BORDER_STATUS)
        .value(VAL_TOP)
        .run()?;

    SetOption::new()
        .target(session_name)
        .option(OPT_PANE_BORDER_FORMAT)
        .value(PANE_BORDER_FORMAT)
        .run()?;

    SetOption::new()
        .target(session_name)
        .option(OPT_PANE_ACTIVE_BORDER_STYLE)
        .value(&format!("fg={}", BARREL_COLOR))
        .run()?;

    SetOption::new()
        .target(session_name)
        .option(OPT_STATUS_STYLE)
        .value(&format!("bg={},fg=#000000", BARREL_COLOR))
        .run()?;

    SetOption::new()
        .window()
        .target(session_name)
        .option(OPT_ALLOW_RENAME)
        .value(VAL_OFF)
        .run()?;

    SetOption::new()
        .target(session_name)
        .option(OPT_STATUS_RIGHT)
        .value(&format!(" barrel v{} ", env!("CARGO_PKG_VERSION")))
        .run()?;

    // Fix mouse behavior after copy
    bind_key(
        KEY_TABLE_COPY_MODE,
        KEY_MOUSE_DRAG_END,
        &["send-keys", "-X", "copy-pipe-and-cancel"],
    )?;

    // Slow down mouse wheel scroll
    bind_key(
        KEY_TABLE_COPY_MODE,
        KEY_WHEEL_UP,
        &["send-keys", "-X", "scroll-up"],
    )
    .ok();
    bind_key(
        KEY_TABLE_COPY_MODE,
        KEY_WHEEL_DOWN,
        &["send-keys", "-X", "scroll-down"],
    )
    .ok();

    rename_window(session_name, &config.workspace)?;

    // Track pane IDs per column and collect all panes for later configuration
    let mut col_first_ids: HashMap<u32, String> = HashMap::new();
    let mut col_last_ids: HashMap<u32, String> = HashMap::new();
    let mut all_panes: Vec<(String, ResolvedPane)> = Vec::new();

    // Get first pane ID and send command if needed
    let first_pane_target = format!("{}:0.0", session_name);
    let first_id = get_pane_id(&first_pane_target)?;

    if let Some(cmd) = build_pane_command(first_pane, workspace_dir.as_deref(), index.as_ref()) {
        std::thread::sleep(std::time::Duration::from_millis(200));
        send_keys(&first_id, &cmd)?;
    }
    col_first_ids.insert(0, first_id.clone());
    col_last_ids.insert(0, first_id.clone());
    all_panes.push((first_id, first_pane.clone()));

    let mut pane_counter = 1;

    // Create columns (horizontal splits)
    for col in 1..=max_col {
        let Some(col_panes) = columns.get(&col) else {
            continue;
        };
        let first_col_pane = col_panes[0];

        let path = first_col_pane
            .path()
            .map(expand_path)
            .unwrap_or_else(|| ".".to_string());

        let wrapper = create_wrapper_script(pane_counter, first_col_pane)?;

        let prev_col = col - 1;
        let target_id = col_first_ids.get(&prev_col).unwrap();

        let mut split = SplitWindow::new()
            .target(target_id)
            .horizontal()
            .start_directory(&path)
            .command(&wrapper);

        if let Some(width) = col_widths.get(&col) {
            split = split.percentage(*width);
        }

        let new_id = split.run()?;
        all_panes.push((new_id.clone(), first_col_pane.clone()));

        if let Some(cmd) =
            build_pane_command(first_col_pane, workspace_dir.as_deref(), index.as_ref())
        {
            std::thread::sleep(std::time::Duration::from_millis(200));
            send_keys(&new_id, &cmd)?;
        }

        col_first_ids.insert(col, new_id.clone());
        col_last_ids.insert(col, new_id);
        pane_counter += 1;
    }

    // Create rows within each column (vertical splits)
    for col in 0..=max_col {
        let Some(col_panes) = columns.get(&col) else {
            continue;
        };

        let num_rows = col_panes.len();

        for (row_idx, &pane) in col_panes.iter().enumerate().skip(1) {
            let path = pane
                .path()
                .map(expand_path)
                .unwrap_or_else(|| ".".to_string());

            let wrapper = create_wrapper_script(pane_counter, pane)?;

            let target_id = col_last_ids.get(&col).unwrap();

            let height_pct = pane.height.unwrap_or_else(|| {
                let remaining = num_rows - row_idx;
                (remaining as u32 * 100) / (remaining as u32 + 1)
            });

            let new_id = SplitWindow::new()
                .target(target_id)
                .vertical()
                .percentage(height_pct)
                .start_directory(&path)
                .command(&wrapper)
                .run()?;

            all_panes.push((new_id.clone(), pane.clone()));

            if let Some(cmd) = build_pane_command(pane, workspace_dir.as_deref(), index.as_ref()) {
                std::thread::sleep(std::time::Duration::from_millis(200));
                send_keys(&new_id, &cmd)?;
            }

            col_last_ids.insert(col, new_id);
            pane_counter += 1;
        }
    }

    // Wait for all shells to initialize, then configure panes
    std::thread::sleep(std::time::Duration::from_millis(500));
    for (pane_id, pane) in &all_panes {
        configure_pane(pane_id, pane)?;
    }

    // Select first pane
    SelectPane::new()
        .target(&format!("{}:0.0", session_name))
        .run()?;

    Ok(())
}

/// Configure a pane's title and background color.
///
/// Called after all panes are created to set visual properties. The title
/// appears in the pane border, and the background color is set if configured.
fn configure_pane(target: &str, pane: &ResolvedPane) -> Result<()> {
    let mut select = SelectPane::new().target(target).title(&pane.name);

    if let Some(color) = pane.color() {
        let tmux_color = to_tmux_color(color);
        if tmux_color != "default" {
            select = select.background(tmux_color);
        }
    }

    select.run()
}

/// Create a temporary bash wrapper script for a pane.
///
/// The wrapper script:
/// 1. Clears the terminal
/// 2. Displays pane notes (if configured) or a simple title
/// 3. Removes itself from disk (self-cleaning)
/// 4. Execs into fish shell with greeting and title disabled
///
/// This approach allows displaying startup information before the shell
/// takes over, while keeping the pane in a clean state.
fn create_wrapper_script(id: usize, pane: &ResolvedPane) -> Result<String> {
    let wrapper_path = format!("/tmp/barrel_ws_{}", id);
    let mut file = std::fs::File::create(&wrapper_path)?;

    writeln!(file, "#!/bin/bash")?;
    writeln!(file, "clear")?;

    let fg_rgb = pane.color().map(to_fg_rgb).unwrap_or("255;255;255");

    if !pane.notes().is_empty() {
        writeln!(file, "COLS=$(tput cols)")?;
        writeln!(file, "printf '\\e[38;2;{}m'", fg_rgb)?;

        let first_note = pane.notes().first().map(|s| s.trim()).unwrap_or("");
        let first_note_len = first_note.chars().count();
        writeln!(
            file,
            "printf ' notes | {}%*s\\n' \"$((COLS - {} - 10))\" ''",
            first_note.replace('\'', "'\\''"),
            first_note_len
        )?;

        for note in pane.notes().iter().skip(1) {
            let note = note.trim();
            let note_len = note.chars().count();
            writeln!(
                file,
                "printf '       | {}%*s\\n' \"$((COLS - {} - 10))\" ''",
                note.replace('\'', "'\\''"),
                note_len
            )?;
        }

        writeln!(file, "printf '\\e[0m'")?;
    } else {
        writeln!(
            file,
            "printf '%b\\n' $'\\e'\"[38;2;{}m- {} -\"$'\\e'\"[0m\"",
            fg_rgb, pane.name
        )?;
    }

    writeln!(file, "rm '{}'", wrapper_path)?;
    writeln!(
        file,
        "exec fish -C 'set fish_greeting; function fish_title; end'"
    )?;

    drop(file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper_path, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(wrapper_path)
}
