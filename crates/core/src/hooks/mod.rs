//! Claude Code hooks configuration.
//!
//! Provides utilities for generating Claude settings.json with hooks
//! that send events to the axel event server.

mod settings;

pub use settings::{
    ClaudeSettings, Hook, HookMatcher, HooksConfig, generate_hooks_settings, settings_path,
    write_settings,
};
