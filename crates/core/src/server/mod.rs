//! Axel Event Server
//!
//! HTTP server that receives Claude Code hook events and OTEL telemetry data,
//! logging everything to a JSONL file.

mod events;
mod logger;
mod routes;

use std::{
    collections::HashMap, net::SocketAddr, path::PathBuf, process::Command, sync::Arc,
    time::Duration,
};

use anyhow::Result;
pub use events::{
    HookEvent, HookEventType, OtelEventType, OutboxResponse, OutboxResponseType, TimestampedEvent,
};
pub use logger::EventLogger;
pub use routes::{AppState, create_router};
use tokio::{
    net::TcpListener,
    sync::{RwLock, broadcast, watch},
};

/// Configuration for the event server
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Port to listen on
    pub port: u16,
    /// Tmux session name to monitor for shutdown
    pub session: String,
    /// Path to the JSONL log file
    pub log_path: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 4318,
            session: String::new(),
            log_path: PathBuf::from(".axel/events.jsonl"),
        }
    }
}

/// Run the event server
pub async fn run_server(config: ServerConfig) -> Result<()> {
    // Create the event logger
    let logger = EventLogger::new(config.log_path.clone()).await?;

    // Create broadcast channel for SSE subscribers (buffer 100 events)
    let (inbox_tx, _) = broadcast::channel(100);

    // Create app state with the logger's sender and broadcast channel
    let tmux_session = if config.session.is_empty() {
        None
    } else {
        Some(config.session.clone())
    };

    let state = AppState {
        event_tx: logger.sender(),
        inbox_tx,
        tmux_session,
        session_to_pane: Arc::new(RwLock::new(HashMap::new())),
    };

    // Build the router
    let app = create_router(state);

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Start the session watchdog if a session is specified
    if !config.session.is_empty() {
        let session = config.session.clone();
        let tx = shutdown_tx.clone();
        tokio::spawn(async move {
            session_watchdog(session, tx).await;
        });
    }

    // Bind to the port
    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = TcpListener::bind(addr).await?;

    // Run the server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_rx))
        .await?;

    eprintln!("Event server shutting down");
    Ok(())
}

/// Watch for tmux session termination
async fn session_watchdog(session: String, shutdown_tx: watch::Sender<bool>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;

        // Check if the session still exists
        let output = Command::new("tmux")
            .args(["has-session", "-t", &session])
            .output();

        match output {
            Ok(result) if !result.status.success() => {
                // Session no longer exists, trigger shutdown
                eprintln!("Tmux session '{}' ended, shutting down server", session);
                let _ = shutdown_tx.send(true);
                break;
            }
            Err(e) => {
                eprintln!("Failed to check tmux session: {}", e);
                // Continue watching in case of transient errors
            }
            _ => {
                // Session still exists, continue watching
            }
        }
    }
}

/// Shutdown signal handler
async fn shutdown_signal(mut rx: watch::Receiver<bool>) {
    // Wait for either Ctrl+C or the watchdog to signal shutdown
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            eprintln!("Received Ctrl+C, initiating shutdown");
        }
        _ = rx.changed() => {
            // Watchdog signaled shutdown
        }
    }
}
