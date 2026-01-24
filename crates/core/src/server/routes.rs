//! Axum route handlers for the event server.

use std::convert::Infallible;
use std::process::Command;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
    Router,
};
use futures_util::stream::Stream;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::events::{HookEvent, OtelEventType, OutboxResponse, TimestampedEvent};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub event_tx: mpsc::Sender<TimestampedEvent>,
    pub inbox_tx: broadcast::Sender<TimestampedEvent>,
    /// Tmux session name for sending responses back to Claude
    pub tmux_session: Option<String>,
}

/// Build the router with all routes
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/inbox", get(handle_inbox_sse))
        .route("/outbox", post(handle_outbox))
        .route("/events/{pane_id}", post(handle_hook_event))
        .route("/v1/metrics", post(handle_otel_metrics))
        .route("/v1/traces", post(handle_otel_traces))
        .route("/v1/logs", post(handle_otel_logs))
        .with_state(Arc::new(state))
}

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

/// SSE endpoint for inbox events
async fn handle_inbox_sse(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.inbox_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        match result {
            Ok(event) => {
                // Serialize the event to JSON
                match serde_json::to_string(&event) {
                    Ok(json) => Some(Ok(Event::default().data(json))),
                    Err(_) => None,
                }
            }
            Err(_) => None, // Skip lagged messages
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Handle Claude Code hook events
async fn handle_hook_event(
    State(state): State<Arc<AppState>>,
    Path(pane_id): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Try to parse as a HookEvent to get the event type
    let event_type = match serde_json::from_value::<HookEvent>(payload.clone()) {
        Ok(hook_event) => hook_event.event_type.to_string(),
        Err(_) => "unknown_hook".to_string(),
    };

    let event = TimestampedEvent::new(event_type, pane_id, payload);

    // Send to file logger
    if state.event_tx.send(event.clone()).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to log event");
    }

    // Broadcast to SSE subscribers (ignore errors if no subscribers)
    let _ = state.inbox_tx.send(event);

    (StatusCode::OK, "OK")
}

/// Handle outbox responses from macOS app
async fn handle_outbox(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<OutboxResponse>,
) -> impl IntoResponse {
    let event_type = payload.response_type.to_string();
    let session_id = payload.session_id.clone();
    let response_text = payload.response_text.clone();

    // Convert to JSON value for storage
    let json_payload = match serde_json::to_value(&payload) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid payload"),
    };

    // Use session_id as the pane_id for outbox responses
    let event = TimestampedEvent::new(event_type, session_id.clone(), json_payload);

    // Send to file logger
    if state.event_tx.send(event.clone()).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to log event");
    }

    // Broadcast to SSE subscribers (so other clients can see the response)
    let _ = state.inbox_tx.send(event);

    // Inject the response into the Claude process
    if let Some(ref tmux_session) = state.tmux_session {
        // Tmux mode: send keys to the appropriate pane
        let target = if let Some(ref pane_id) = payload.pane_id {
            pane_id.clone()
        } else {
            // Default to first pane in the session (pane 0.0)
            // Skip pane 0 which is the server pane, target pane 1
            format!("{}:0.1", tmux_session)
        };

        // Send the response text followed by Enter
        let result = Command::new("tmux")
            .args(["send-keys", "-t", &target, &response_text, "Enter"])
            .output();

        if let Err(e) = result {
            eprintln!("[outbox] Failed to send keys to tmux: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to send response to tmux");
        }
    } else {
        // Non-tmux mode: write response to a file
        let response_dir = std::path::PathBuf::from(".axel");
        let response_file = response_dir.join(format!("response_{}.txt", session_id));

        // Ensure directory exists
        if let Err(e) = std::fs::create_dir_all(&response_dir) {
            eprintln!("[outbox] Failed to create response directory: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to write response file");
        }

        // Write the response
        if let Err(e) = std::fs::write(&response_file, &response_text) {
            eprintln!("[outbox] Failed to write response file: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to write response file");
        }
    }

    (StatusCode::OK, "OK")
}

/// Handle OTEL metrics
async fn handle_otel_metrics(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    handle_otel_event(state, OtelEventType::Metrics, payload).await
}

/// Handle OTEL traces
async fn handle_otel_traces(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    handle_otel_event(state, OtelEventType::Traces, payload).await
}

/// Handle OTEL logs
async fn handle_otel_logs(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    handle_otel_event(state, OtelEventType::Logs, payload).await
}

/// Common handler for OTEL events
async fn handle_otel_event(
    state: Arc<AppState>,
    event_type: OtelEventType,
    payload: serde_json::Value,
) -> impl IntoResponse {
    // OTEL events use "otel" as the pane_id since they're not associated with a specific terminal
    let event = TimestampedEvent::new(event_type.to_string(), "otel", payload);

    // Send to file logger
    if state.event_tx.send(event.clone()).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to log event");
    }

    // Broadcast to SSE subscribers (ignore errors if no subscribers)
    let _ = state.inbox_tx.send(event);

    (StatusCode::OK, "OK")
}
