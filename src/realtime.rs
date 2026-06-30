use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

#[cfg(test)]
use crate::{EndpointBindingValueFormat, RuntimeDiagnostic, ValueOccurrenceHeader};
use crate::{RuntimeSessionRecord, RuntimeSessionSnapshot, runtime_time::created_at_now};

mod graph_command;
mod node_catalog;
mod presence;
mod protocol;
mod state;
mod wire;

use graph_command::handle_graph_command;
pub(crate) use node_catalog::node_catalog_snapshot_for_record;
use node_catalog::{
    NodeCatalogHelloRequest, handle_node_catalog_request, hello_node_catalog_payload,
};
use presence::{handle_presence_update, handle_selection_update};
use protocol::*;
pub use protocol::{
    RUNTIME_REALTIME_REPLAY_LIMIT, RUNTIME_REALTIME_SCHEMA, RUNTIME_REALTIME_SCHEMA_VERSION,
};
pub use state::RuntimeRealtimeState;
use state::{RuntimeRealtimeCachedCommandResult, sync_required_diagnostic};
use wire::{RuntimeRealtimeConnectionIdentity, RuntimeRealtimeSessionRevisions};
pub use wire::{RuntimeRealtimeDiagnostic, RuntimeRealtimeEnvelope, RuntimeRealtimeReplay};

type RuntimeRealtimeSocketSender = futures_util::stream::SplitSink<WebSocket, Message>;

pub async fn handle_runtime_realtime_socket(record: RuntimeSessionRecord, socket: WebSocket) {
    let mut receiver = record.realtime.subscribe();
    let (mut sender, mut socket_receiver) = socket.split();
    let mut identity: Option<RuntimeRealtimeConnectionIdentity> = None;
    let mut high_water_sequence = 0;

    'socket_loop: loop {
        tokio::select! {
            Some(message) = socket_receiver.next() => {
                let message = match message {
                    Ok(message) => message,
                    Err(_) => break,
                };
                match message {
                    Message::Text(text) => {
                        let parsed = serde_json::from_str::<RuntimeRealtimeEnvelope>(&text);
                        let frame = match parsed {
                            Ok(frame) => frame,
                            Err(error) => {
                                let diagnostic = runtime_error(&record.id, None, None, "realtime.frame.invalid-json", format!("invalid realtime JSON frame: {error}"), None);
                                if send_frame(&mut sender, &diagnostic).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        };

                        if frame.session_id != record.id {
                            let diagnostic = runtime_error(&record.id, None, Some(&frame), "realtime.session.mismatch", "frame sessionId does not match the WebSocket session", Some(json!({"expectedSessionId": record.id, "actualSessionId": frame.session_id})));
                            if send_frame(&mut sender, &diagnostic).await.is_err() {
                                break;
                            }
                            continue;
                        }

                        match frame.message_type.as_str() {
                            FRAME_SESSION_HELLO => {
                                let snapshot = current_snapshot(&record);
                                let hello = decode_hello_payload(&frame);
                                let resumed_identity = match hello.resume_token.as_deref() {
                                    Some(resume_token) => {
                                        match record.realtime.consume_resume_token(resume_token) {
                                            Ok(identity) => Some(identity),
                                            Err(diagnostic) => {
                                                let issued_identity =
                                                    record.realtime.issue_connection_identity(None);
                                                high_water_sequence =
                                                    record.realtime.current_sequence();
                                                let sync = session_sync_required(
                                                    &record,
                                                    &issued_identity,
                                                    &frame,
                                                    &snapshot,
                                                    diagnostic,
                                                    hello.node_catalog.as_ref(),
                                                );
                                                if send_frame(&mut sender, &sync).await.is_err() {
                                                    break;
                                                }
                                                identity = Some(issued_identity);
                                                continue;
                                            }
                                        }
                                    }
                                    None => None,
                                };
                                let issued_identity =
                                    record.realtime.issue_connection_identity(resumed_identity);
                                match hello.last_cursor.as_deref() {
                                    Some(last_cursor) => {
                                        match record.realtime.replay_after(last_cursor) {
                                            Ok(replay) => {
                                                high_water_sequence = replay.high_water_sequence;
                                                let attached = session_attached(&record, &issued_identity, &frame, &snapshot, hello.node_catalog.as_ref());
                                                if send_frame(&mut sender, &attached).await.is_err() {
                                                    break;
                                                }
                                                for event in replay.events {
                                                    if send_frame(&mut sender, &event).await.is_err() {
                                                        break 'socket_loop;
                                                    }
                                                }
                                            }
                                            Err(diagnostic) => {
                                                high_water_sequence = record.realtime.current_sequence();
                                                let sync = session_sync_required(&record, &issued_identity, &frame, &snapshot, diagnostic, hello.node_catalog.as_ref());
                                                if send_frame(&mut sender, &sync).await.is_err() {
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    None => {
                                        high_water_sequence = record.realtime.current_sequence();
                                        let attached = session_attached(&record, &issued_identity, &frame, &snapshot, hello.node_catalog.as_ref());
                                        if send_frame(&mut sender, &attached).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                identity = Some(issued_identity);
                            }
                            FRAME_PRESENCE_UPDATE => {
                                let Some(identity) = identity.as_ref() else {
                                    if send_not_attached(&record, &mut sender, &frame)
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                };
                                match handle_presence_update(&record, identity, frame) {
                                    Ok((ack, event)) => {
                                        if send_frame(&mut sender, &ack).await.is_err() {
                                            break;
                                        }
                                        if let Some(event) = event {
                                            record.realtime.publish(event);
                                        }
                                    }
                                    Err(diagnostic) => {
                                        if send_realtime_diagnostic(
                                            &record,
                                            &mut sender,
                                            identity,
                                            diagnostic,
                                        )
                                        .await
                                        .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                            }
                            FRAME_SELECTION_UPDATE => {
                                let Some(identity) = identity.as_ref() else {
                                    if send_not_attached(&record, &mut sender, &frame)
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                };
                                match handle_selection_update(&record, identity, frame) {
                                    Ok((ack, event)) => {
                                        if send_frame(&mut sender, &ack).await.is_err() {
                                            break;
                                        }
                                        if let Some(event) = event {
                                            record.realtime.publish(event);
                                        }
                                    }
                                    Err(diagnostic) => {
                                        if send_realtime_diagnostic(
                                            &record,
                                            &mut sender,
                                            identity,
                                            diagnostic,
                                        )
                                        .await
                                        .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                            }
                            FRAME_CONTROL_COMMAND => {
                                let Some(identity) = identity.as_ref() else {
                                    if send_not_attached(&record, &mut sender, &frame)
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                };
                                let diagnostic = runtime_error(
                                    &record.id,
                                    Some(identity),
                                    Some(&frame),
                                    "realtime.control-command.disabled",
                                    "control.command is disabled; send live control input through graph.command kind node.input",
                                    Some(json!({
                                        "replacementType": FRAME_GRAPH_COMMAND,
                                        "replacementKind": GRAPH_KIND_NODE_INPUT,
                                    })),
                                );
                                if send_frame(&mut sender, &diagnostic).await.is_err() {
                                    break;
                                }
                            }
                            FRAME_GRAPH_COMMAND => {
                                let Some(identity) = identity.as_ref() else {
                                    if send_not_attached(&record, &mut sender, &frame)
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                };
                                match handle_graph_command(&record, identity, frame) {
                                    Ok((ack, events, local_events)) => {
                                        if send_frame(&mut sender, &ack).await.is_err() {
                                            break;
                                        }
                                        for local_event in local_events {
                                            if send_frame(&mut sender, &local_event).await.is_err()
                                            {
                                                break 'socket_loop;
                                            }
                                        }
                                        for event in events {
                                            record.realtime.publish(event);
                                        }
                                    }
                                    Err(diagnostic) => {
                                        if send_realtime_diagnostic(
                                            &record,
                                            &mut sender,
                                            identity,
                                            diagnostic,
                                        )
                                        .await
                                        .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                            }
                            FRAME_NODE_CATALOG_REQUEST => {
                                let Some(identity) = identity.as_ref() else {
                                    if send_not_attached(&record, &mut sender, &frame)
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                };
                                match handle_node_catalog_request(&record, identity, frame) {
                                    Ok(response) => {
                                        if send_frame(&mut sender, &response).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(diagnostic) => {
                                        if send_realtime_diagnostic(
                                            &record,
                                            &mut sender,
                                            identity,
                                            diagnostic,
                                        )
                                        .await
                                        .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                            }
                            _ => {
                                let diagnostic = runtime_error(&record.id, identity.as_ref(), Some(&frame), "realtime.frame.unsupported-type", "unsupported Runtime realtime frame type", Some(json!({"type": frame.message_type})));
                                if send_frame(&mut sender, &diagnostic).await.is_err() {
                                    break;
                                }
                            }
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
                        let diagnostic = runtime_error(&record.id, identity.as_ref(), None, "realtime.frame.binary-unsupported", "Runtime realtime frames must be JSON text", None);
                        if send_frame(&mut sender, &diagnostic).await.is_err() {
                            break;
                        }
                    }
                }
            }
            event = receiver.recv() => {
                match event {
                    Ok(event) if event.sequence.unwrap_or_default() <= high_water_sequence => {}
                    Ok(event) => {
                        if send_frame(&mut sender, &event).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let Some(identity) = identity.as_ref() else {
                            continue;
                        };
                        let snapshot = current_snapshot(&record);
                        let diagnostic = sync_required_diagnostic(
                            "realtime.cursor.stream-lagged",
                            "WebSocket receiver lagged beyond the Runtime realtime event window",
                            Some(json!({ "currentCursor": record.realtime.current_cursor() })),
                        );
                        let sync = session_sync_required(&record, identity, &empty_correlation_frame(&record.id), &snapshot, diagnostic, None);
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

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelloPayload {
    last_cursor: Option<String>,
    resume_token: Option<String>,
    #[serde(default)]
    node_catalog: Option<NodeCatalogHelloRequest>,
}

fn decode_hello_payload(frame: &RuntimeRealtimeEnvelope) -> HelloPayload {
    let mut hello =
        serde_json::from_value::<HelloPayload>(frame.payload.clone()).unwrap_or_default();
    if hello.last_cursor.is_none() {
        hello.last_cursor = frame.cursor.clone();
    }
    hello
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
            "accepted": true,
            "cached": cached,
            "eventCursor": event_cursor,
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

fn graph_ack_with_payload(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: Value,
) -> RuntimeRealtimeEnvelope {
    ack_with_payload(record, identity, frame, EVENT_GRAPH_ACK, payload)
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
    diagnostic: RuntimeRealtimeDiagnostic,
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
            "diagnostic": diagnostic,
        }),
    }
}

fn runtime_error(
    session_id: &str,
    identity: Option<&RuntimeRealtimeConnectionIdentity>,
    frame: Option<&RuntimeRealtimeEnvelope>,
    code: &str,
    message: impl Into<String>,
    details: Option<Value>,
) -> RuntimeRealtimeEnvelope {
    let diagnostic = RuntimeRealtimeDiagnostic {
        code: code.to_owned(),
        message: message.into(),
        details,
    };
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "runtime.error".to_owned(),
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
        payload: json!({ "diagnostic": diagnostic }),
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
) -> Result<&'a EndpointBindingValueFormat, RuntimeDiagnostic> {
    if let Err(report) = skenion_contracts::validate_value_occurrence_header_v01(header) {
        return Err(RuntimeDiagnostic::structured_error(
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
        return Err(RuntimeDiagnostic::structured_error(
            "runtime.value-binding.unknown-binding",
            "value occurrence header references an unknown binding",
            json!({
                "bindingId": header.binding_id,
            }),
        ));
    };

    if binding_format.binding_epoch != header.binding_epoch {
        return Err(RuntimeDiagnostic::structured_error(
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
        return Err(RuntimeDiagnostic::structured_error(
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

async fn send_not_attached(
    record: &RuntimeSessionRecord,
    sender: &mut RuntimeRealtimeSocketSender,
    frame: &RuntimeRealtimeEnvelope,
) -> Result<(), axum::Error> {
    let diagnostic = runtime_error(
        &record.id,
        None,
        Some(frame),
        "realtime.session.not-attached",
        "send session.hello before client actions",
        None,
    );
    send_frame(sender, &diagnostic).await
}

async fn send_realtime_diagnostic(
    record: &RuntimeSessionRecord,
    sender: &mut RuntimeRealtimeSocketSender,
    identity: &RuntimeRealtimeConnectionIdentity,
    diagnostic: RuntimeRealtimeDiagnostic,
) -> Result<(), axum::Error> {
    let diagnostic = runtime_error(
        &record.id,
        Some(identity),
        None,
        &diagnostic.code,
        diagnostic.message,
        diagnostic.details,
    );
    send_frame(sender, &diagnostic).await
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
