//! Axum route handlers for the event server.

use std::{collections::HashMap, convert::Infallible, process::Command, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::stream::Stream;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio_stream::{StreamExt, wrappers::BroadcastStream};

use super::events::{HookEvent, OtelEventType, OutboxResponse, TimestampedEvent};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub event_tx: mpsc::Sender<TimestampedEvent>,
    pub inbox_tx: broadcast::Sender<TimestampedEvent>,
    /// Tmux session name for sending responses back to Claude
    pub tmux_session: Option<String>,
    /// Mapping from Claude session_id to pane_id (for correlating OTEL metrics)
    pub session_to_pane: Arc<RwLock<HashMap<String, String>>>,
}

/// Build the router with all routes
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/inbox", get(handle_inbox_sse))
        .route("/outbox", post(handle_outbox))
        .route("/events/{pane_id}", post(handle_hook_event))
        // OTEL routes with pane_id for direct correlation
        .route("/v1/metrics/{pane_id}", post(handle_otel_metrics_with_pane))
        .route("/v1/traces/{pane_id}", post(handle_otel_traces_with_pane))
        .route("/v1/logs/{pane_id}", post(handle_otel_logs_with_pane))
        // Legacy OTEL routes without pane_id (fallback)
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

    // Extract session_id from payload and store mapping for OTEL correlation
    if let Some(session_id) = payload.get("session_id").and_then(|v| v.as_str()) {
        let mut mapping = state.session_to_pane.write().await;
        mapping.insert(session_id.to_string(), pane_id.clone());
    } else {
        // Log what keys ARE in the payload for debugging
        let _keys: Vec<&str> = payload
            .as_object()
            .map(|obj| obj.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();
    }

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

        // Send the response text literally (handles special chars, spaces, newlines)
        let text_result = Command::new("tmux")
            .args(["send-keys", "-t", &target, "-l", &response_text])
            .output();

        if let Err(e) = text_result {
            eprintln!("[outbox] Failed to send text to tmux: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to send response to tmux",
            );
        }

        // Send Enter key to submit the prompt
        let enter_result = Command::new("tmux")
            .args(["send-keys", "-t", &target, "Enter"])
            .output();

        if let Err(e) = enter_result {
            eprintln!("[outbox] Failed to send Enter to tmux: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to send response to tmux",
            );
        }
    } else {
        // Non-tmux mode: write response to a file
        let response_dir = std::path::PathBuf::from(".axel");
        let response_file = response_dir.join(format!("response_{}.txt", session_id));

        // Ensure directory exists
        if let Err(e) = std::fs::create_dir_all(&response_dir) {
            eprintln!("[outbox] Failed to create response directory: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to write response file",
            );
        }

        // Write the response
        if let Err(e) = std::fs::write(&response_file, &response_text) {
            eprintln!("[outbox] Failed to write response file: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to write response file",
            );
        }
    }

    (StatusCode::OK, "OK")
}

/// Handle OTEL metrics with pane_id in URL
async fn handle_otel_metrics_with_pane(
    State(state): State<Arc<AppState>>,
    Path(pane_id): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    handle_otel_event_with_pane(state, OtelEventType::Metrics, pane_id, payload).await
}

/// Handle OTEL traces with pane_id in URL
async fn handle_otel_traces_with_pane(
    State(state): State<Arc<AppState>>,
    Path(pane_id): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    handle_otel_event_with_pane(state, OtelEventType::Traces, pane_id, payload).await
}

/// Handle OTEL logs with pane_id in URL
async fn handle_otel_logs_with_pane(
    State(state): State<Arc<AppState>>,
    Path(pane_id): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    handle_otel_event_with_pane(state, OtelEventType::Logs, pane_id, payload).await
}

/// Handle OTEL metrics (legacy, without pane_id)
async fn handle_otel_metrics(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    handle_otel_event(state, OtelEventType::Metrics, payload).await
}

/// Handle OTEL traces (legacy, without pane_id)
async fn handle_otel_traces(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    handle_otel_event(state, OtelEventType::Traces, payload).await
}

/// Handle OTEL logs (legacy, without pane_id)
async fn handle_otel_logs(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    handle_otel_event(state, OtelEventType::Logs, payload).await
}

/// OTEL handler with pane_id directly from URL
async fn handle_otel_event_with_pane(
    state: Arc<AppState>,
    event_type: OtelEventType,
    pane_id: String,
    payload: serde_json::Value,
) -> impl IntoResponse {
    eprintln!(
        "[otel] Received {} with pane_id from URL: {}",
        event_type,
        &pane_id[..8.min(pane_id.len())]
    );

    let event = TimestampedEvent::new(event_type.to_string(), pane_id, payload);

    // Send to file logger
    if state.event_tx.send(event.clone()).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to log event");
    }

    // Broadcast to SSE subscribers
    let _ = state.inbox_tx.send(event);

    (StatusCode::OK, "OK")
}

/// Common handler for OTEL events
async fn handle_otel_event(
    state: Arc<AppState>,
    event_type: OtelEventType,
    payload: serde_json::Value,
) -> impl IntoResponse {
    // Try to extract session.id from OTEL payload to find the corresponding pane_id
    let session_id_opt = extract_otel_session_id(&payload);

    let pane_id = if let Some(ref session_id) = session_id_opt {
        let mapping = state.session_to_pane.blocking_read();
        if let Some(pane) = mapping.get(session_id) {
            eprintln!(
                "[otel] Found pane mapping for session {}: {}",
                &session_id[..8.min(session_id.len())],
                &pane[..8.min(pane.len())]
            );
            pane.clone()
        } else {
            eprintln!(
                "[otel] No pane mapping for session {}. Registered sessions: {:?}",
                &session_id[..8.min(session_id.len())],
                mapping
                    .keys()
                    .map(|k| &k[..8.min(k.len())])
                    .collect::<Vec<_>>()
            );
            "otel".to_string()
        }
    } else {
        eprintln!("[otel] Could not extract session.id from payload");
        "otel".to_string()
    };

    let event = TimestampedEvent::new(event_type.to_string(), pane_id, payload);

    // Send to file logger
    if state.event_tx.send(event.clone()).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to log event");
    }

    // Broadcast to SSE subscribers (ignore errors if no subscribers)
    let _ = state.inbox_tx.send(event);

    (StatusCode::OK, "OK")
}

/// Extract session.id from OTEL metrics payload
fn extract_otel_session_id(payload: &serde_json::Value) -> Option<String> {
    // OTEL metrics structure: resourceMetrics[].scopeMetrics[].metrics[].sum.dataPoints[].attributes[]
    // We need to find attributes with key="session.id"
    let resource_metrics = payload.get("resourceMetrics")?.as_array()?;

    for rm in resource_metrics {
        let scope_metrics = rm.get("scopeMetrics")?.as_array()?;
        for sm in scope_metrics {
            let metrics = sm.get("metrics")?.as_array()?;
            for metric in metrics {
                if let Some(sum) = metric.get("sum")
                    && let Some(data_points) = sum.get("dataPoints").and_then(|d| d.as_array())
                {
                    for dp in data_points {
                        if let Some(attributes) = dp.get("attributes").and_then(|a| a.as_array()) {
                            for attr in attributes {
                                if attr.get("key").and_then(|k| k.as_str()) == Some("session.id")
                                    && let Some(value) = attr.get("value")
                                    && let Some(s) =
                                        value.get("stringValue").and_then(|v| v.as_str())
                                {
                                    return Some(s.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}
