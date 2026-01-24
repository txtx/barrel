//! Server command for running the axel event server.

use std::path::PathBuf;

use anyhow::Result;
use axel_core::server::{ServerConfig, run_server};
use clap::Args;

/// Server command arguments
#[derive(Debug, Clone, Args)]
pub struct ServerArgs {
    /// Port to listen on
    #[arg(short, long, default_value = "4318")]
    pub port: u16,

    /// Tmux session name to monitor for auto-shutdown
    #[arg(short, long)]
    pub session: String,

    /// Path to the JSONL log file
    #[arg(short, long)]
    pub log: PathBuf,
}

/// Run the server command
pub async fn run(args: ServerArgs) -> Result<()> {
    let config = ServerConfig {
        port: args.port,
        session: args.session,
        log_path: args.log,
    };

    run_server(config).await
}
