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

    /// Tmux session name to monitor for auto-shutdown (optional for standalone mode)
    #[arg(short, long)]
    pub session: Option<String>,

    /// Path to the JSONL log file
    #[arg(short, long, default_value = ".axel/events.jsonl")]
    pub log: PathBuf,
}

/// Run the server command
pub async fn run(args: ServerArgs) -> Result<()> {
    let config = ServerConfig {
        port: args.port,
        session: args.session.unwrap_or_default(),
        log_path: args.log,
    };

    eprintln!("Starting axel event server on port {}", config.port);
    eprintln!("Logging to: {:?}", config.log_path);
    if !config.session.is_empty() {
        eprintln!("Monitoring tmux session: {}", config.session);
    } else {
        eprintln!("Running in standalone mode (no tmux session monitoring)");
    }

    run_server(config).await
}
