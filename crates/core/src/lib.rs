//! Barrel Core - Core library for the barrel workspace launcher
//!
//! This crate provides the core functionality for barrel including:
//! - Configuration parsing and types
//! - Tmux session management
//! - Agent driver implementations
//! - Claude command building

pub mod claude;
pub mod config;
pub mod drivers;
pub mod tmux;

// Re-export commonly used types at crate root
pub use config::{
    Agent, AgentPathConfig, AiShellConfig, CustomShellConfig, Profile, ProfilePane, ProfileType,
    ResolvedPane, ShellConfig, TerminalConfig, WorkspaceConfig, WorkspaceIndex,
};
pub use drivers::{AgentDriver, ClaudeDriver, CodexDriver, OpenCodeDriver, all_agent_patterns};
