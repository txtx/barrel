//! Event type definitions for the axel event server.
//!
//! Handles Claude Code hook events and OTEL telemetry data.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Wrapper for timestamped events logged to JSONL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimestampedEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: String,
    /// The pane ID (UUID) that identifies which terminal/pane this event came from.
    /// This is extracted from the URL path `/events/{pane_id}`.
    pub pane_id: String,
    pub event: serde_json::Value,
}

impl TimestampedEvent {
    pub fn new(
        event_type: impl Into<String>,
        pane_id: impl Into<String>,
        event: serde_json::Value,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            event_type: event_type.into(),
            pane_id: pane_id.into(),
            event,
        }
    }
}

/// Claude Code hook event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEventType {
    PreToolUse,
    PostToolUse,
    SessionStart,
    SessionEnd,
    Stop,
    SubagentStop,
    PermissionRequest,
}

impl std::fmt::Display for HookEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookEventType::PreToolUse => write!(f, "PreToolUse"),
            HookEventType::PostToolUse => write!(f, "PostToolUse"),
            HookEventType::SessionStart => write!(f, "SessionStart"),
            HookEventType::SessionEnd => write!(f, "SessionEnd"),
            HookEventType::Stop => write!(f, "Stop"),
            HookEventType::SubagentStop => write!(f, "SubagentStop"),
            HookEventType::PermissionRequest => write!(f, "PermissionRequest"),
        }
    }
}

/// Claude Code hook event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEvent {
    #[serde(rename = "type")]
    pub event_type: HookEventType,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

/// OTEL telemetry event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtelEventType {
    Metrics,
    Traces,
    Logs,
}

impl std::fmt::Display for OtelEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OtelEventType::Metrics => write!(f, "otel_metrics"),
            OtelEventType::Traces => write!(f, "otel_traces"),
            OtelEventType::Logs => write!(f, "otel_logs"),
        }
    }
}

/// Outbox response from macOS app (permission responses, answers, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxResponse {
    /// The Claude session ID this response is for
    pub session_id: String,
    /// Type of response
    pub response_type: OutboxResponseType,
    /// The actual response text to send (e.g., "y" for allow, "n" for deny)
    pub response_text: String,
    /// Optional tmux pane ID to target (if not provided, uses session's first pane)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
}

/// Types of outbox responses
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxResponseType {
    /// Response to a permission request (allow/deny)
    PermissionResponse,
    /// Response to a question/hint
    QuestionResponse,
}

impl std::fmt::Display for OutboxResponseType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutboxResponseType::PermissionResponse => write!(f, "permission_response"),
            OutboxResponseType::QuestionResponse => write!(f, "question_response"),
        }
    }
}
