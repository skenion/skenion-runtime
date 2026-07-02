use axum::{
    Json,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy)]
enum DisabledLiveChannel {
    SessionEvents,
    Mutate,
    Operation,
    Operations,
    CollaborationPresence,
    CollaborationSelection,
    CollaborationEvents,
    Undo,
    Redo,
    ControlEvent,
}

impl DisabledLiveChannel {
    fn replacement(self) -> Value {
        match self {
            Self::SessionEvents => json!({
                "type": "session.hello",
                "details": "Use the WebSocket session and resume with payload.lastCursor for replay."
            }),
            Self::Mutate => json!({
                "type": "graph.command",
                "kinds": ["view.patch", "graph.changeSet"]
            }),
            Self::Operation => json!({
                "type": "graph.command",
                "kind": "graph.pasteFragment"
            }),
            Self::Operations => json!({
                "type": "graph.command",
                "kinds": ["graph.changeSet", "graph.pasteFragment", "history.undo", "history.redo"]
            }),
            Self::CollaborationPresence => json!({
                "type": "selection.update",
                "details": "Runtime realtime presence frames are not part of the current public surface."
            }),
            Self::CollaborationSelection => json!({ "type": "selection.update" }),
            Self::CollaborationEvents => json!({
                "type": "session.hello",
                "details": "Use WebSocket realtime events instead of collaboration SSE."
            }),
            Self::Undo => json!({
                "type": "graph.command",
                "kind": "history.undo"
            }),
            Self::Redo => json!({
                "type": "graph.command",
                "kind": "history.redo"
            }),
            Self::ControlEvent => json!({
                "type": "node.input"
            }),
        }
    }
}

pub(crate) async fn session_events_stream(Path(session_id): Path<String>) -> Response {
    response(&session_id, DisabledLiveChannel::SessionEvents)
}

pub(crate) async fn mutate(Path(session_id): Path<String>) -> Response {
    response(&session_id, DisabledLiveChannel::Mutate)
}

pub(crate) async fn operation(Path(session_id): Path<String>) -> Response {
    response(&session_id, DisabledLiveChannel::Operation)
}

pub(crate) async fn operations(Path(session_id): Path<String>) -> Response {
    response(&session_id, DisabledLiveChannel::Operations)
}

pub(crate) async fn collaboration_presence(Path(session_id): Path<String>) -> Response {
    response(&session_id, DisabledLiveChannel::CollaborationPresence)
}

pub(crate) async fn collaboration_selection(Path(session_id): Path<String>) -> Response {
    response(&session_id, DisabledLiveChannel::CollaborationSelection)
}

pub(crate) async fn collaboration_events_stream(Path(session_id): Path<String>) -> Response {
    response(&session_id, DisabledLiveChannel::CollaborationEvents)
}

pub(crate) async fn undo(Path(session_id): Path<String>) -> Response {
    response(&session_id, DisabledLiveChannel::Undo)
}

pub(crate) async fn redo(Path(session_id): Path<String>) -> Response {
    response(&session_id, DisabledLiveChannel::Redo)
}

pub(crate) async fn control_event(Path(session_id): Path<String>) -> Response {
    response(&session_id, DisabledLiveChannel::ControlEvent)
}

fn response(session_id: &str, channel: DisabledLiveChannel) -> Response {
    (
        StatusCode::GONE,
        Json(json!({
            "ok": false,
            "schema": "skenion.runtime.http-live-channel-disabled",
            "schemaVersion": "0.1.0",
            "sessionId": session_id,
            "issues": [{
                "severity": "error",
                "code": "runtime.http-live-channel-disabled",
                "message": "HTTP live mutation and event channels are disabled; use the session WebSocket instead.",
                "details": {
                    "websocketEndpoint": format!("/v0/sessions/{session_id}"),
                    "replacement": channel.replacement()
                }
            }]
        })),
    )
        .into_response()
}
