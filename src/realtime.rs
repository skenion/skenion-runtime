use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::broadcast;

#[cfg(test)]
use crate::{EndpointBindingValueFormat, RuntimeIssue, ValueOccurrenceHeader};
use crate::{RuntimeSessionRecord, RuntimeSessionSnapshot, runtime_time::created_at_now};

mod control_input;
mod graph_command;
mod node_catalog;
mod node_input;
mod presence;
mod protocol;
mod session_engine;
mod state;
mod wire;

pub(crate) use node_catalog::node_catalog_snapshot_for_record;
use node_catalog::{NodeCatalogHelloRequest, hello_node_catalog_payload};
use protocol::*;
pub use protocol::{
    RUNTIME_REALTIME_REPLAY_LIMIT, RUNTIME_REALTIME_SCHEMA, RUNTIME_REALTIME_SCHEMA_VERSION,
};
pub(in crate::realtime) use session_engine::RealtimeDispatch;
use session_engine::RuntimeRealtimeSessionEngine;
use state::RuntimeRealtimeCachedCommandResult;
pub use state::RuntimeRealtimeState;
use wire::{RuntimeRealtimeConnectionIdentity, RuntimeRealtimeSessionRevisions};
pub use wire::{RuntimeRealtimeEnvelope, RuntimeRealtimeIssue, RuntimeRealtimeReplay};

type RuntimeRealtimeSocketSender = futures_util::stream::SplitSink<WebSocket, Message>;

pub async fn handle_runtime_realtime_socket(record: RuntimeSessionRecord, socket: WebSocket) {
    let mut engine = RuntimeRealtimeSessionEngine::new(record);
    let mut receiver = engine.subscribe();
    let (mut sender, mut socket_receiver) = socket.split();

    loop {
        tokio::select! {
            Some(message) = socket_receiver.next() => {
                let message = match message {
                    Ok(message) => message,
                    Err(_) => break,
                };
                match message {
                    Message::Text(text) => {
                        let output = engine.handle_text_frame(&text);
                        if send_session_output(&mut sender, &output).await.is_err() {
                            break;
                        }
                        for event in output.broadcast_events {
                            engine.publish(event);
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(payload) => {
                        if sender.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Message::Pong(_) => {}
                    Message::Binary(_) => {
                        let issue = engine.binary_unsupported_issue();
                        if send_frame(&mut sender, &issue).await.is_err() {
                            break;
                        }
                    }
                }
            }
            event = receiver.recv() => {
                match event {
                    Ok(event) => {
                        let Some(event) = engine.handle_broadcast_event(event) else {
                            continue;
                        };
                        if send_frame(&mut sender, &event).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let Some(sync) = engine.handle_lagged_receiver() else {
                            continue;
                        };
                        if send_frame(&mut sender, &sync).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

fn command_ack(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    event_cursor: &str,
    cached: bool,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: EVENT_COMMAND_ACK.to_owned(),
        message_id: format!("{}_ack_{}", record.id, frame.message_id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: frame.idempotency_key.clone(),
        sequence: None,
        cursor: Some(record.realtime.current_cursor()),
        created_at: Some(created_at_now()),
        payload: json!({
            "status": "accepted",
            "accepted": true,
            "applied": false,
            "conflict": false,
            "cached": cached,
            "eventCursor": event_cursor,
            "issues": [],
        }),
    }
}

fn command_ack_from_cached(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    cached: RuntimeRealtimeCachedCommandResult,
) -> RuntimeRealtimeEnvelope {
    command_ack_with_payload(
        record,
        identity,
        frame,
        mark_ack_payload_cached(cached.ack_payload),
    )
}

fn command_ack_with_payload(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: Value,
) -> RuntimeRealtimeEnvelope {
    ack_with_payload(record, identity, frame, EVENT_COMMAND_ACK, payload)
}

fn ack_with_payload(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    message_type: &str,
    payload: Value,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: message_type.to_owned(),
        message_id: format!("{}_ack_{}", record.id, frame.message_id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: frame.idempotency_key.clone(),
        sequence: None,
        cursor: Some(record.realtime.current_cursor()),
        created_at: Some(created_at_now()),
        payload,
    }
}

fn mark_ack_payload_cached(mut payload: Value) -> Value {
    if let Some(object) = payload.as_object_mut() {
        object.insert("cached".to_owned(), Value::Bool(true));
    }
    payload
}

fn session_attached(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    snapshot: &RuntimeSessionSnapshot,
    node_catalog: Option<&NodeCatalogHelloRequest>,
) -> RuntimeRealtimeEnvelope {
    let node_catalog = hello_node_catalog_payload(record, node_catalog);
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "session.attached".to_owned(),
        message_id: format!("{}_attached_{}", record.id, identity.connection_id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: None,
        correlation_id: Some(frame.message_id.clone()),
        idempotency_key: None,
        sequence: None,
        cursor: Some(record.realtime.current_cursor()),
        created_at: Some(created_at_now()),
        payload: json!({
            "connectionId": identity.connection_id,
            "clientId": identity.client_id,
            "windowId": identity.window_id,
            "resumeToken": identity.resume_token,
            "currentRevisions": current_revisions(snapshot),
            "snapshot": snapshot,
            "globalCursor": record.realtime.current_cursor(),
            "nodeCatalog": node_catalog,
        }),
    }
}

fn session_sync_required(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    snapshot: &RuntimeSessionSnapshot,
    issue: RuntimeRealtimeIssue,
    node_catalog: Option<&NodeCatalogHelloRequest>,
) -> RuntimeRealtimeEnvelope {
    let node_catalog = hello_node_catalog_payload(record, node_catalog);
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "session.syncRequired".to_owned(),
        message_id: format!("{}_sync_required_{}", record.id, identity.connection_id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: None,
        correlation_id: Some(frame.message_id.clone()),
        idempotency_key: None,
        sequence: None,
        cursor: Some(record.realtime.current_cursor()),
        created_at: Some(created_at_now()),
        payload: json!({
            "connectionId": identity.connection_id,
            "clientId": identity.client_id,
            "windowId": identity.window_id,
            "resumeToken": identity.resume_token,
            "currentRevisions": current_revisions(snapshot),
            "snapshot": snapshot,
            "globalCursor": record.realtime.current_cursor(),
            "nodeCatalog": node_catalog,
            "issue": issue,
        }),
    }
}

fn runtime_issue(
    session_id: &str,
    identity: Option<&RuntimeRealtimeConnectionIdentity>,
    frame: Option<&RuntimeRealtimeEnvelope>,
    code: &str,
    message: impl Into<String>,
    details: Option<Value>,
) -> RuntimeRealtimeEnvelope {
    let issue = json!({
        "severity": "error",
        "code": code,
        "message": message.into(),
        "details": details,
    });
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: EVENT_RUNTIME_ISSUE.to_owned(),
        message_id: format!(
            "{}_error_{}",
            session_id,
            created_at_now().replace(':', "_")
        ),
        session_id: session_id.to_owned(),
        connection_id: identity.map(|identity| identity.connection_id.clone()),
        client_id: identity.map(|identity| identity.client_id.clone()),
        window_id: identity.map(|identity| identity.window_id.clone()),
        command_id: frame.and_then(|frame| frame.command_id.clone()),
        correlation_id: frame.map(|frame| frame.message_id.clone()),
        idempotency_key: frame.and_then(|frame| frame.idempotency_key.clone()),
        sequence: None,
        cursor: None,
        created_at: Some(created_at_now()),
        payload: json!({ "issue": issue }),
    }
}

fn empty_correlation_frame(session_id: &str) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "runtime.internal".to_owned(),
        message_id: "runtime-internal".to_owned(),
        session_id: session_id.to_owned(),
        connection_id: None,
        client_id: None,
        window_id: None,
        command_id: None,
        correlation_id: None,
        idempotency_key: None,
        sequence: None,
        cursor: None,
        created_at: None,
        payload: Value::Null,
    }
}

fn current_snapshot(record: &RuntimeSessionRecord) -> RuntimeSessionSnapshot {
    record
        .session
        .read()
        .expect("runtime session lock should not be poisoned")
        .snapshot()
}

fn current_revisions(snapshot: &RuntimeSessionSnapshot) -> RuntimeRealtimeSessionRevisions {
    RuntimeRealtimeSessionRevisions {
        session_revision: snapshot.session_revision,
        view_revision: snapshot.view_revision,
        control_revision: snapshot.control_revision,
        graph_revision: snapshot.graph_revision().map(ToOwned::to_owned),
    }
}

#[cfg(test)]
pub(crate) fn validate_value_occurrence_header_for_session_binding<'a>(
    header: &ValueOccurrenceHeader,
    binding_formats: &'a [EndpointBindingValueFormat],
) -> Result<&'a EndpointBindingValueFormat, RuntimeIssue> {
    if let Err(report) = skenion_contracts::validate_value_occurrence_header_v01(header) {
        return Err(RuntimeIssue::structured_error(
            "runtime.value-binding.invalid-header",
            "invalid value occurrence header",
            json!({
                "bindingId": header.binding_id,
                "errors": report
                    .errors()
                    .iter()
                    .map(|error| error.message.clone())
                    .collect::<Vec<_>>(),
            }),
        ));
    }

    let Some(binding_format) = binding_formats
        .iter()
        .find(|binding_format| binding_format.binding_id == header.binding_id)
    else {
        return Err(RuntimeIssue::structured_error(
            "runtime.value-binding.unknown-binding",
            "value occurrence header references an unknown binding",
            json!({
                "bindingId": header.binding_id,
            }),
        ));
    };

    if binding_format.binding_epoch != header.binding_epoch {
        return Err(RuntimeIssue::structured_error(
            "runtime.value-binding.stale-epoch",
            "value occurrence header binding epoch does not match the current binding",
            json!({
                "bindingId": header.binding_id,
                "expectedBindingEpoch": binding_format.binding_epoch,
                "receivedBindingEpoch": header.binding_epoch,
            }),
        ));
    }

    if binding_format.format_revision != header.format_revision {
        return Err(RuntimeIssue::structured_error(
            "runtime.value-binding.stale-format-revision",
            "value occurrence header format revision does not match the current binding",
            json!({
                "bindingId": header.binding_id,
                "expectedFormatRevision": binding_format.format_revision,
                "receivedFormatRevision": header.format_revision,
            }),
        ));
    }

    Ok(binding_format)
}

async fn send_session_output(
    sender: &mut RuntimeRealtimeSocketSender,
    output: &RealtimeDispatch,
) -> Result<(), axum::Error> {
    for frame in &output.direct_frames {
        send_frame(sender, frame).await?;
    }
    Ok(())
}

async fn send_frame(
    sender: &mut RuntimeRealtimeSocketSender,
    frame: &RuntimeRealtimeEnvelope,
) -> Result<(), axum::Error> {
    sender
        .send(Message::Text(
            serde_json::to_string(frame)
                .expect("runtime realtime frame should serialize")
                .into(),
        ))
        .await
}

fn unix_ms_timestamp(time: SystemTime) -> String {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("unix-ms:{millis}")
}

#[cfg(test)]
mod tests;
