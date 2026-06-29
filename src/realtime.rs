use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use skenion_contracts::{
    InterfaceIncidentEdgePolicyV01, NodeCatalogSnapshotV01, PackageChecksumV01,
};
use tokio::sync::broadcast;

use crate::{
    CanvasNodeView, ControlMessage, ControlValue, DiagnosticSeverity, GraphTargetRef,
    PasteGraphFragmentRequest, PasteGraphFragmentResponse, PatchPath, RuntimeCollaborationChange,
    RuntimeCollaborationCursor, RuntimeCollaborationSelection,
    RuntimeCollaborationSelectionEnvelope, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeDiagnostic, RuntimeMutationRequest, RuntimeOperationAttribution,
    RuntimeOperationDiagnostic, RuntimeOperationEnvelope, RuntimePatchResponse,
    RuntimeSessionRecord, RuntimeSessionSnapshot, RuntimeViewPatch,
    object_text::{
        ObjectRegistry, ObjectTextPortActivation, ObjectTextPortDirection, ObjectTextPortRate,
        ObjectTextResolution, materialize_object_text_node_v01,
        materialize_unresolved_object_text_node_v01, object_text_node_definition_v01,
        unresolved_object_text_node_definition_v01,
    },
    runtime_time::created_at_now,
    session::{ApplyObjectNodeCreateCurrentRequest, ApplyObjectNodeReplaceCurrentRequest},
    validate_runtime_collaboration_selection_envelope,
};
#[cfg(test)]
use crate::{EndpointBindingValueFormat, ValueOccurrenceHeader};

mod protocol;
mod state;
mod wire;

use protocol::*;
pub use protocol::{
    RUNTIME_REALTIME_REPLAY_LIMIT, RUNTIME_REALTIME_SCHEMA, RUNTIME_REALTIME_SCHEMA_VERSION,
};
pub use state::RuntimeRealtimeState;
use state::{RememberAckInput, RuntimeRealtimeCachedCommandResult, sync_required_diagnostic};
#[cfg(test)]
use state::{RuntimeRealtimeIdempotencyScope, RuntimeRealtimeResumeIdentity};
use wire::{RuntimeRealtimeConnectionIdentity, RuntimeRealtimeSessionRevisions};
pub use wire::{RuntimeRealtimeDiagnostic, RuntimeRealtimeEnvelope, RuntimeRealtimeReplay};

type RuntimeRealtimeSocketSender = futures_util::stream::SplitSink<WebSocket, Message>;

#[derive(Clone, Copy)]
struct RealtimeEventPosition<'a> {
    sequence: u64,
    cursor: &'a str,
}

struct GraphEventContext<'a> {
    record: &'a RuntimeSessionRecord,
    identity: &'a RuntimeRealtimeConnectionIdentity,
    frame: &'a RuntimeRealtimeEnvelope,
    command: &'a GraphCommandPayload,
    response: &'a RuntimePatchResponse,
    node_result: Option<&'a Value>,
    operation_result: Option<&'a PasteGraphFragmentResponse>,
    position: RealtimeEventPosition<'a>,
}

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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeCatalogHelloRequest {
    #[serde(default)]
    mode: NodeCatalogHelloMode,
    #[serde(default)]
    known_revision: Option<Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum NodeCatalogHelloMode {
    #[default]
    None,
    IfChanged,
    Always,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeCatalogRequestPayload {
    #[serde(default)]
    known_revision: Option<Value>,
}

fn decode_hello_payload(frame: &RuntimeRealtimeEnvelope) -> HelloPayload {
    let mut hello =
        serde_json::from_value::<HelloPayload>(frame.payload.clone()).unwrap_or_default();
    if hello.last_cursor.is_none() {
        hello.last_cursor = frame.cursor.clone();
    }
    hello
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresenceUpdatePayload {
    #[serde(default)]
    presence: Value,
    #[serde(default)]
    ttl_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectionUpdatePayload {
    target: GraphTargetRef,
    selection: RuntimeCollaborationSelection,
    #[serde(default)]
    cursor: Option<RuntimeCollaborationCursor>,
    #[serde(default)]
    ttl_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphCommandPayload {
    kind: String,
    #[serde(default)]
    base_session_revision: Option<u64>,
    #[serde(default)]
    base_graph_revision: Option<String>,
    #[serde(default)]
    base_view_revision: Option<u64>,
    #[serde(default)]
    target: Option<GraphTargetRef>,
    #[serde(default)]
    view_patch: Option<RuntimeViewPatch>,
    #[serde(default)]
    changes: Option<Vec<RuntimeCollaborationChange>>,
    #[serde(default)]
    object_text: Option<String>,
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    requested_node_id: Option<String>,
    #[serde(default)]
    view: Option<CanvasNodeView>,
    #[serde(default)]
    params: Option<Map<String, Value>>,
    #[serde(default)]
    port_id: Option<String>,
    #[serde(default)]
    message: Option<ControlMessage>,
    #[serde(default)]
    request: Option<PasteGraphFragmentRequest>,
    #[serde(default)]
    scope: Option<HistoryCommandScope>,
    #[serde(default)]
    unresolved_policy: Option<ObjectUnresolvedPolicy>,
    #[serde(default)]
    interface_incident_edge_policy: Option<InterfaceIncidentEdgePolicyV01>,
    #[serde(default)]
    surface_path: Option<Value>,
    #[serde(default)]
    description: Option<String>,
}

impl GraphCommandPayload {
    fn command_kind(&self) -> Option<GraphCommandKind> {
        GraphCommandKind::parse(&self.kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum HistoryCommandScope {
    Client,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ObjectUnresolvedPolicy {
    Reject,
    MaterializeDiagnostic,
}

#[derive(Debug)]
struct GraphCommandOutcome {
    response: RuntimePatchResponse,
    node_result: Option<Value>,
    operation_result: Option<PasteGraphFragmentResponse>,
    control_emission: Option<GraphControlEmission>,
    catalog_snapshot: Option<NodeCatalogSnapshotV01>,
}

#[derive(Debug)]
struct GraphControlEmission {
    request: RuntimeControlEventRequest,
    response: RuntimeControlEventResponse,
    changed_values: BTreeMap<String, ControlValue>,
}

fn handle_presence_update(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<(RuntimeRealtimeEnvelope, Option<RuntimeRealtimeEnvelope>), RuntimeRealtimeDiagnostic> {
    let idempotency_key = frame.idempotency_key.clone().ok_or_else(|| {
        sync_required_diagnostic(
            "realtime.command.idempotency-key-required",
            "presence.update requires idempotencyKey",
            None,
        )
    })?;
    if let Some(cached) =
        record
            .realtime
            .cached_command_result(identity, &frame.message_type, &idempotency_key)
    {
        return Ok((
            command_ack_from_cached(record, identity, &frame, cached),
            None,
        ));
    }

    let payload = serde_json::from_value::<PresenceUpdatePayload>(frame.payload.clone()).map_err(
        |error| {
            sync_required_diagnostic(
                "realtime.presence.invalid-payload",
                format!("invalid presence.update payload: {error}"),
                None,
            )
        },
    )?;
    let ttl_ms = payload.ttl_ms.unwrap_or(30_000).clamp(1_000, 300_000);
    let now = SystemTime::now();
    let updated_at = unix_ms_timestamp(now);
    let expires_at_time = now + Duration::from_millis(ttl_ms);
    let expires_at = unix_ms_timestamp(expires_at_time);
    let sequence = record.realtime.next_event_sequence();
    let cursor = record.realtime.cursor_for(sequence);
    let presence_payload = json!({
        "presence": payload.presence,
        "ttlMs": ttl_ms,
        "updatedAt": updated_at,
        "expiresAt": expires_at,
        "ephemeral": true,
        "replayed": false,
    });
    let presence = json!({
        "sessionId": record.id,
        "connectionId": identity.connection_id,
        "clientId": identity.client_id,
        "windowId": identity.window_id,
        "presence": presence_payload,
    });
    record
        .realtime
        .remember_presence(identity, presence.clone(), expires_at_time, sequence);

    let event = RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: EVENT_PRESENCE_UPDATED.to_owned(),
        message_id: format!("{}_presence_{sequence:06}", record.id),
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
        idempotency_key: Some(idempotency_key.clone()),
        sequence: Some(sequence),
        cursor: Some(cursor.clone()),
        created_at: Some(created_at_now()),
        payload: presence,
    };
    let ack = command_ack(record, identity, &frame, &cursor, false);
    record.realtime.remember_ack(RememberAckInput {
        identity,
        message_type: &frame.message_type,
        idempotency_key: &idempotency_key,
        event_cursor: &cursor,
        event_sequence: sequence,
        ack_payload: ack.payload.clone(),
        emitted_results: Vec::new(),
    });
    Ok((ack, Some(event)))
}

fn handle_selection_update(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<(RuntimeRealtimeEnvelope, Option<RuntimeRealtimeEnvelope>), RuntimeRealtimeDiagnostic> {
    let idempotency_key = frame.idempotency_key.clone().ok_or_else(|| {
        sync_required_diagnostic(
            "realtime.command.idempotency-key-required",
            "selection.update requires idempotencyKey",
            None,
        )
    })?;
    if let Some(cached) =
        record
            .realtime
            .cached_command_result(identity, &frame.message_type, &idempotency_key)
    {
        return Ok((
            command_ack_from_cached(record, identity, &frame, cached),
            None,
        ));
    }

    let payload = serde_json::from_value::<SelectionUpdatePayload>(frame.payload.clone()).map_err(
        |error| {
            sync_required_diagnostic(
                "realtime.selection.invalid-payload",
                format!("invalid selection.update payload: {error}"),
                None,
            )
        },
    )?;
    let ttl_ms = payload.ttl_ms.unwrap_or(30_000).clamp(1_000, 300_000);
    let now = SystemTime::now();
    let updated_at = unix_ms_timestamp(now);
    let expires_at_time = now + Duration::from_millis(ttl_ms);
    let expires_at = unix_ms_timestamp(expires_at_time);
    let selection = RuntimeCollaborationSelectionEnvelope {
        schema: "skenion.runtime.collaboration.selection".to_owned(),
        schema_version: "0.1.0".to_owned(),
        session_id: record.id.clone(),
        participant_id: identity.client_id.clone(),
        target: payload.target,
        selection: payload.selection,
        cursor: payload.cursor,
        updated_at,
        expires_at,
    };
    if let Err(report) = validate_runtime_collaboration_selection_envelope(&selection) {
        return Err(sync_required_diagnostic(
            "realtime.selection.invalid-selection",
            format!("invalid selection.update selection: {report}"),
            None,
        ));
    }

    let sequence = record.realtime.next_event_sequence();
    let cursor = record.realtime.cursor_for(sequence);
    let event = RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: EVENT_SELECTION_UPDATED.to_owned(),
        message_id: format!("{}_selection_{sequence:06}", record.id),
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
        idempotency_key: Some(idempotency_key.clone()),
        sequence: Some(sequence),
        cursor: Some(cursor.clone()),
        created_at: Some(created_at_now()),
        payload: json!({
            "sessionId": record.id,
            "connectionId": identity.connection_id,
            "clientId": identity.client_id,
            "windowId": identity.window_id,
            "selection": selection,
            "ttlMs": ttl_ms,
            "ephemeral": true,
            "replayed": false,
        }),
    };
    let ack = command_ack(record, identity, &frame, &cursor, false);
    record.realtime.remember_ack(RememberAckInput {
        identity,
        message_type: &frame.message_type,
        idempotency_key: &idempotency_key,
        event_cursor: &cursor,
        event_sequence: sequence,
        ack_payload: ack.payload.clone(),
        emitted_results: Vec::new(),
    });
    Ok((ack, Some(event)))
}

fn handle_node_catalog_request(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<RuntimeRealtimeEnvelope, RuntimeRealtimeDiagnostic> {
    let request = serde_json::from_value::<NodeCatalogRequestPayload>(frame.payload.clone())
        .map_err(|error| {
            sync_required_diagnostic(
                "realtime.node-catalog.invalid-payload",
                format!("invalid nodeCatalog.request payload: {error}"),
                None,
            )
        })?;
    let snapshot = node_catalog_snapshot_for_record(record);
    let (message_type, payload) =
        if catalog_revision_matches(request.known_revision.as_ref(), &snapshot.catalog_revision) {
            (
                "nodeCatalog.unchanged",
                node_catalog_unchanged_response_payload(snapshot),
            )
        } else {
            (
                "nodeCatalog.snapshot",
                node_catalog_snapshot_response_payload(snapshot),
            )
        };
    Ok(node_catalog_response(
        record,
        identity,
        &frame,
        message_type,
        payload,
    ))
}

fn handle_graph_command(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<
    (
        RuntimeRealtimeEnvelope,
        Vec<RuntimeRealtimeEnvelope>,
        Vec<RuntimeRealtimeEnvelope>,
    ),
    RuntimeRealtimeDiagnostic,
> {
    let idempotency_key = frame.idempotency_key.clone().ok_or_else(|| {
        sync_required_diagnostic(
            "realtime.command.idempotency-key-required",
            "graph.command requires idempotencyKey",
            None,
        )
    })?;
    if let Some(cached) =
        record
            .realtime
            .cached_command_result(identity, &frame.message_type, &idempotency_key)
    {
        let emitted_results = cached.emitted_results.clone();
        return Ok((
            graph_ack_from_cached(record, identity, &frame, cached),
            Vec::new(),
            emitted_results,
        ));
    }

    let payload =
        serde_json::from_value::<GraphCommandPayload>(frame.payload.clone()).map_err(|error| {
            sync_required_diagnostic(
                "realtime.graph.invalid-payload",
                format!("invalid graph.command payload: {error}"),
                None,
            )
        })?;
    let sequence = record.realtime.next_event_sequence();
    let cursor = record.realtime.cursor_for(sequence);
    let outcome = apply_graph_command(record, identity, &frame, &payload);
    let GraphCommandOutcome {
        response,
        node_result,
        operation_result,
        control_emission,
        catalog_snapshot,
    } = outcome;
    let position = RealtimeEventPosition {
        sequence,
        cursor: &cursor,
    };
    let graph_context = GraphEventContext {
        record,
        identity,
        frame: &frame,
        command: &payload,
        response: &response,
        node_result: node_result.as_ref(),
        operation_result: operation_result.as_ref(),
        position,
    };
    let ack = graph_ack(&graph_context, false);
    let event = if response.applied {
        Some(graph_applied_event(&graph_context))
    } else if let Some(mut control_emission) = control_emission {
        if control_emission.response.ok {
            control_emitted_event(
                record,
                identity,
                &frame,
                &control_emission.request,
                &mut control_emission.response,
                control_emission.changed_values,
                position,
            )
        } else {
            None
        }
    } else {
        None
    };
    let mut events = event.into_iter().collect::<Vec<_>>();
    if let Some(catalog_snapshot) = catalog_snapshot {
        let catalog_sequence = record.realtime.next_event_sequence();
        let catalog_cursor = record.realtime.cursor_for(catalog_sequence);
        events.push(node_catalog_changed_event(
            record,
            identity,
            &frame,
            catalog_snapshot,
            catalog_sequence,
            catalog_cursor,
        ));
    }
    record.realtime.remember_ack(RememberAckInput {
        identity,
        message_type: &frame.message_type,
        idempotency_key: &idempotency_key,
        event_cursor: &cursor,
        event_sequence: sequence,
        ack_payload: ack.payload.clone(),
        emitted_results: events.clone(),
    });

    Ok((ack, events, Vec::new()))
}

fn apply_graph_command(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let before = session.snapshot();
    let before_catalog_revision = node_catalog_snapshot_for_session(&session).catalog_revision;

    if let Some(base_session_revision) = payload.base_session_revision
        && base_session_revision != before.session_revision
    {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            &session,
            true,
            RuntimeDiagnostic::structured_error(
                "graph.command.session-revision-conflict",
                format!(
                    "baseSessionRevision {base_session_revision} does not match session revision {}",
                    before.session_revision
                ),
                json!({
                    "expectedRevision": base_session_revision,
                    "actualRevision": before.session_revision,
                    "commandKind": payload.kind,
                }),
            ),
        ));
    }

    let Some(command_kind) = payload.command_kind() else {
        let supported_kinds = graph_command_supported_kind_names();
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            &session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.kind-unsupported",
                format!(
                    "unsupported graph.command kind {}; supported kinds are {}",
                    payload.kind,
                    supported_kinds.join(", ")
                ),
                json!({
                    "kind": payload.kind,
                    "supportedKinds": supported_kinds,
                }),
            ),
        ));
    };

    if command_kind == GraphCommandKind::NodeInput {
        let Some(node_id) = payload.node_id.clone() else {
            return GraphCommandOutcome::with_node_result(
                graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.node-id-required",
                        "graph.command kind node.input requires payload.nodeId",
                        json!({ "commandKind": payload.kind }),
                    ),
                ),
                node_command_result(payload, None, None, Vec::new(), None),
            );
        };
        let Some(port_id) = payload.port_id.clone() else {
            return GraphCommandOutcome::with_node_result(
                graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.port-id-required",
                        "graph.command kind node.input requires payload.portId",
                        json!({ "commandKind": payload.kind, "nodeId": node_id }),
                    ),
                ),
                node_command_result(payload, None, Some(&node_id), Vec::new(), None),
            );
        };
        let Some(message) = payload.message.clone() else {
            return GraphCommandOutcome::with_node_result(
                graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.message-required",
                        "graph.command kind node.input requires payload.message",
                        json!({
                            "commandKind": payload.kind,
                            "nodeId": node_id,
                            "portId": port_id,
                        }),
                    ),
                ),
                node_command_result(payload, None, Some(&node_id), Vec::new(), None),
            );
        };
        drop(session);
        let request = RuntimeControlEventRequest {
            node_id,
            port_id,
            message,
        };
        let (response, changed_values, applied_request) = apply_control_command(record, request);
        let (snapshot, history) = {
            let session = record
                .session
                .read()
                .expect("runtime session lock should not be poisoned");
            (session.snapshot(), session.history())
        };
        let input = node_input_result(&applied_request, &response);
        let patch_response = RuntimePatchResponse {
            ok: response.ok,
            applied: false,
            conflict: false,
            snapshot,
            history,
            diagnostics: response.diagnostics.clone(),
        };
        let control_emission = response.ok.then_some(GraphControlEmission {
            request: applied_request.clone(),
            response,
            changed_values,
        });
        return GraphCommandOutcome::with_node_result_and_control_emission(
            patch_response,
            node_command_result(
                payload,
                None,
                Some(&applied_request.node_id),
                Vec::new(),
                Some(input),
            ),
            control_emission,
        );
    }

    let response = match command_kind {
        GraphCommandKind::ViewPatch => {
            let Some(view_patch) = payload.view_patch.clone() else {
                return GraphCommandOutcome::from_response(graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.view-patch-required",
                        "graph.command kind view.patch requires payload.viewPatch",
                        json!({ "commandKind": payload.kind }),
                    ),
                ));
            };
            if let Some(base_view_revision) = payload.base_view_revision
                && base_view_revision != view_patch.base_view_revision
            {
                return GraphCommandOutcome::from_response(graph_command_rejected_response(
                    &session,
                    true,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.view-revision-conflict",
                        format!(
                            "baseViewRevision {base_view_revision} does not match viewPatch.baseViewRevision {}",
                            view_patch.base_view_revision
                        ),
                        json!({
                            "expectedRevision": base_view_revision,
                            "actualRevision": view_patch.base_view_revision,
                            "commandKind": payload.kind,
                        }),
                    ),
                ));
            }
            if let Some(base_graph_revision) = payload.base_graph_revision.as_deref() {
                let actual_graph_revision = before.graph_revision().map(ToOwned::to_owned);
                if actual_graph_revision.as_deref() != Some(base_graph_revision) {
                    return GraphCommandOutcome::from_response(graph_command_rejected_response(
                        &session,
                        true,
                        RuntimeDiagnostic::structured_error(
                            "graph.command.graph-revision-conflict",
                            format!(
                                "baseGraphRevision {base_graph_revision} does not match graph revision {}",
                                actual_graph_revision.as_deref().unwrap_or("none")
                            ),
                            json!({
                                "expectedRevision": base_graph_revision,
                                "actualRevision": actual_graph_revision,
                                "commandKind": payload.kind,
                            }),
                        ),
                    ));
                }
            }
            if let Some(target) = payload.target.as_ref() {
                if !matches!(target.path, PatchPath::Root) {
                    return GraphCommandOutcome::from_response(graph_command_rejected_response(
                        &session,
                        false,
                        RuntimeDiagnostic::structured_error(
                            "graph.command.view-target-unsupported",
                            "view.patch realtime commands currently support only the loaded root graph view",
                            json!({ "target": target, "commandKind": payload.kind }),
                        ),
                    ));
                }
                let actual_target_revision = session.target_revision_current(target);
                if actual_target_revision.as_deref() != Some(target.base_revision.as_str()) {
                    return GraphCommandOutcome::from_response(graph_command_rejected_response(
                        &session,
                        true,
                        RuntimeDiagnostic::structured_error(
                            "graph.command.target-revision-conflict",
                            format!(
                                "target baseRevision {} does not match target graph revision {}",
                                target.base_revision,
                                actual_target_revision.as_deref().unwrap_or("none")
                            ),
                            json!({
                                "expectedRevision": target.base_revision,
                                "actualRevision": actual_target_revision,
                                "target": target,
                                "commandKind": payload.kind,
                            }),
                        ),
                    ));
                }
            }

            session.apply_mutation(RuntimeMutationRequest {
                graph_patch: None,
                view_patch: Some(view_patch),
                actor_id: Some(identity.client_id.clone()),
                client_id: Some(identity.client_id.clone()),
                description: Some(graph_command_description(payload, frame)),
            })
        }
        GraphCommandKind::ChangeSet => {
            let Some(target) = payload.target.clone() else {
                return GraphCommandOutcome::from_response(graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.target-required",
                        "graph.command kind graph.changeSet requires payload.target",
                        json!({ "commandKind": payload.kind }),
                    ),
                ));
            };
            let changes = payload.changes.clone().unwrap_or_default();
            if changes.is_empty() {
                return GraphCommandOutcome::from_response(graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.changes-required",
                        "graph.command kind graph.changeSet requires at least one change",
                        json!({ "target": target, "commandKind": payload.kind }),
                    ),
                ));
            }
            if let Some(base_graph_revision) = payload.base_graph_revision.as_deref()
                && base_graph_revision != target.base_revision
            {
                return GraphCommandOutcome::from_response(graph_command_rejected_response(
                    &session,
                    true,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.target-revision-conflict",
                        format!(
                            "baseGraphRevision {base_graph_revision} does not match target.baseRevision {}",
                            target.base_revision
                        ),
                        json!({
                            "expectedRevision": base_graph_revision,
                            "actualRevision": target.base_revision,
                            "target": target,
                            "commandKind": payload.kind,
                        }),
                    ),
                ));
            }
            session.apply_collaboration_change_set_current(
                target,
                changes,
                Some(identity.client_id.clone()),
                Some(identity.client_id.clone()),
                Some(graph_command_description(payload, frame)),
            )
        }
        GraphCommandKind::PasteFragment => {
            return apply_paste_fragment_graph_command(&mut session, identity, frame, payload)
                .with_catalog_change(before_catalog_revision, &session);
        }
        GraphCommandKind::HistoryUndo => {
            let scope = payload.scope.unwrap_or(HistoryCommandScope::Client);
            match scope {
                HistoryCommandScope::Client => session.undo_for_actor(&identity.client_id),
                HistoryCommandScope::Global => session.undo(),
            }
        }
        GraphCommandKind::HistoryRedo => {
            let scope = payload.scope.unwrap_or(HistoryCommandScope::Client);
            match scope {
                HistoryCommandScope::Client => session.redo_for_actor(&identity.client_id),
                HistoryCommandScope::Global => session.redo(),
            }
        }
        GraphCommandKind::NodeResolve => {
            return apply_object_resolve_graph_command(&session, payload);
        }
        GraphCommandKind::NodeCreate => {
            return apply_object_create_graph_command(&mut session, identity, frame, payload)
                .with_catalog_change(before_catalog_revision, &session);
        }
        GraphCommandKind::NodeReplace => {
            return apply_object_replace_graph_command(&mut session, identity, frame, payload)
                .with_catalog_change(before_catalog_revision, &session);
        }
        GraphCommandKind::NodeDelete => {
            return apply_node_delete_graph_command(&mut session, identity, frame, payload)
                .with_catalog_change(before_catalog_revision, &session);
        }
        GraphCommandKind::NodeUpdate => {
            return apply_node_update_graph_command(&mut session, identity, frame, payload)
                .with_catalog_change(before_catalog_revision, &session);
        }
        GraphCommandKind::NodeInput => {
            unreachable!("node.input is handled before graph mutation commands")
        }
    };
    GraphCommandOutcome::from_response(response)
        .with_catalog_change(before_catalog_revision, &session)
}

fn graph_command_description(
    payload: &GraphCommandPayload,
    frame: &RuntimeRealtimeEnvelope,
) -> String {
    payload
        .description
        .clone()
        .unwrap_or_else(|| format!("Realtime graph command {}", frame.message_id))
}

fn apply_paste_fragment_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let Some(request) = payload.request.clone() else {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.request-required",
                "graph.command kind graph.pasteFragment requires payload.request",
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    if let Some(base_graph_revision) = payload.base_graph_revision.as_deref()
        && base_graph_revision != request.target.base_revision
    {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            true,
            RuntimeDiagnostic::structured_error(
                "graph.command.target-revision-conflict",
                format!(
                    "baseGraphRevision {base_graph_revision} does not match request.target.baseRevision {}",
                    request.target.base_revision
                ),
                json!({
                    "expectedRevision": base_graph_revision,
                    "actualRevision": request.target.base_revision,
                    "target": request.target,
                    "commandKind": payload.kind,
                }),
            ),
        ));
    }
    let operation = RuntimeOperationEnvelope {
        schema: "skenion.runtime.operation".to_owned(),
        schema_version: "0.1.0".to_owned(),
        id: frame
            .command_id
            .clone()
            .unwrap_or_else(|| frame.message_id.clone()),
        kind: "pasteGraphFragment".to_owned(),
        request,
        attribution: Some(RuntimeOperationAttribution {
            actor_id: Some(identity.client_id.clone()),
            client_id: Some(identity.client_id.clone()),
            label: Some(graph_command_description(payload, frame)),
        }),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        created_at: Some(created_at_now()),
    };
    let operation_result = session.apply_runtime_operation(operation);
    let response = RuntimePatchResponse {
        ok: operation_result.ok,
        applied: operation_result.applied,
        conflict: operation_result.conflict,
        snapshot: session.snapshot(),
        history: session.history(),
        diagnostics: operation_diagnostics_to_runtime(&operation_result.diagnostics),
    };
    GraphCommandOutcome::with_operation_result(response, operation_result)
}

fn operation_diagnostics_to_runtime(
    diagnostics: &[RuntimeOperationDiagnostic],
) -> Vec<RuntimeDiagnostic> {
    diagnostics
        .iter()
        .map(|diagnostic| RuntimeDiagnostic {
            severity: match diagnostic.severity.as_str() {
                "warning" => DiagnosticSeverity::Warning,
                "info" => DiagnosticSeverity::Info,
                _ => DiagnosticSeverity::Error,
            },
            message: diagnostic.message.clone(),
            code: Some(diagnostic.code.clone()),
            details: Some(json!({
                "path": diagnostic.path.clone(),
                "target": diagnostic.target.clone(),
                "expectedRevision": diagnostic.expected_revision.clone(),
                "actualRevision": diagnostic.actual_revision.clone(),
                "duplicates": diagnostic.duplicates.clone(),
                "nodes": diagnostic.nodes.clone(),
                "edges": diagnostic.edges.clone(),
                "interfacePolicy": diagnostic.interface_policy.clone(),
                "interfaceDetail": diagnostic.interface_detail.clone(),
            })),
        })
        .collect()
}

impl GraphCommandOutcome {
    fn from_response(response: RuntimePatchResponse) -> Self {
        Self {
            response,
            node_result: None,
            operation_result: None,
            control_emission: None,
            catalog_snapshot: None,
        }
    }

    fn with_operation_result(
        response: RuntimePatchResponse,
        operation_result: PasteGraphFragmentResponse,
    ) -> Self {
        Self {
            response,
            node_result: None,
            operation_result: Some(operation_result),
            control_emission: None,
            catalog_snapshot: None,
        }
    }

    fn with_node_result(response: RuntimePatchResponse, node_result: Value) -> Self {
        Self {
            response,
            node_result: Some(node_result),
            operation_result: None,
            control_emission: None,
            catalog_snapshot: None,
        }
    }

    fn with_node_result_and_control_emission(
        response: RuntimePatchResponse,
        node_result: Value,
        control_emission: Option<GraphControlEmission>,
    ) -> Self {
        Self {
            response,
            node_result: Some(node_result),
            operation_result: None,
            control_emission,
            catalog_snapshot: None,
        }
    }

    fn with_catalog_change(
        mut self,
        before_catalog_revision: PackageChecksumV01,
        session: &crate::RuntimeSession,
    ) -> Self {
        if self.response.applied {
            let snapshot = node_catalog_snapshot_for_session(session);
            if snapshot.catalog_revision != before_catalog_revision {
                self.catalog_snapshot = Some(snapshot);
            }
        }
        self
    }
}

fn resolve_object_command_text(
    session: &crate::RuntimeSession,
    object_text: &str,
) -> ObjectTextResolution {
    let project = session.project_document_current();
    ObjectRegistry::for_project(project.as_ref()).resolve(object_text)
}

fn apply_object_resolve_graph_command(
    session: &crate::RuntimeSession,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let Some(object_text) = payload.object_text.as_deref() else {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.object-text-required",
                "graph.command kind node.resolve requires payload.objectText",
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    let resolution = resolve_object_command_text(session, object_text);
    let node_result = node_command_result(payload, Some(&resolution), None, Vec::new(), None);
    if let Err(response) = validate_object_command_target(session, payload, true) {
        return GraphCommandOutcome::with_node_result(*response, node_result);
    }

    GraphCommandOutcome::with_node_result(
        RuntimePatchResponse {
            ok: true,
            applied: false,
            conflict: false,
            snapshot: session.snapshot(),
            history: session.history(),
            diagnostics: object_text_runtime_diagnostics(&resolution),
        },
        node_result,
    )
}

fn apply_object_create_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let Some(object_text) = payload.object_text.as_deref() else {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.object-text-required",
                "graph.command kind node.create requires payload.objectText",
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    let resolution = resolve_object_command_text(session, object_text);
    let node_result = node_command_result(
        payload,
        Some(&resolution),
        payload.requested_node_id.as_deref(),
        Vec::new(),
        None,
    );
    let target = match validate_object_command_target(session, payload, false) {
        Ok(target) => target,
        Err(response) => return GraphCommandOutcome::with_node_result(*response, node_result),
    };
    let node_id = payload
        .requested_node_id
        .clone()
        .unwrap_or_else(|| generated_node_id_for_create(session, &target, &resolution));
    let Some((node, definition)) =
        materialize_object_command_node(session, payload, &resolution, &node_id)
    else {
        let node_result =
            node_command_result(payload, Some(&resolution), Some(&node_id), Vec::new(), None);
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "node.command.unresolved",
                    "object text could not be resolved for node.create",
                    json!({
                        "commandKind": payload.kind,
                        "target": target,
                        "objectText": object_text,
                        "unresolvedPolicy": object_unresolved_policy(payload),
                        "resolution": object_resolution_json(&resolution),
                    }),
                ),
            ),
            node_result,
        );
    };

    let response = session.apply_object_node_create_current(ApplyObjectNodeCreateCurrentRequest {
        target,
        node,
        view: payload.view.clone(),
        definition: Some(definition),
        mutation: RuntimeMutationRequest {
            graph_patch: None,
            view_patch: None,
            actor_id: Some(identity.client_id.clone()),
            client_id: Some(identity.client_id.clone()),
            description: payload
                .description
                .clone()
                .or_else(|| Some(format!("Realtime graph command {}", frame.message_id))),
        },
    });
    let node_result =
        node_command_result(payload, Some(&resolution), Some(&node_id), Vec::new(), None);
    GraphCommandOutcome::with_node_result(response, node_result)
}

fn apply_object_replace_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let Some(object_text) = payload.object_text.as_deref() else {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.object-text-required",
                "graph.command kind node.replace requires payload.objectText",
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    let resolution = resolve_object_command_text(session, object_text);
    let node_id = payload.node_id.clone();
    let node_result = node_command_result(
        payload,
        Some(&resolution),
        node_id.as_deref(),
        Vec::new(),
        None,
    );
    let target = match validate_object_command_target(session, payload, false) {
        Ok(target) => target,
        Err(response) => return GraphCommandOutcome::with_node_result(*response, node_result),
    };
    let Some(node_id) = node_id else {
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "graph.command.node-id-required",
                    "graph.command kind node.replace requires payload.nodeId",
                    json!({ "commandKind": payload.kind, "target": target }),
                ),
            ),
            node_result,
        );
    };
    let Some((node, definition)) =
        materialize_object_command_node(session, payload, &resolution, &node_id)
    else {
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "node.command.unresolved",
                    "object text could not be resolved for node.replace",
                    json!({
                        "commandKind": payload.kind,
                        "target": target,
                        "nodeId": node_id,
                        "objectText": object_text,
                        "unresolvedPolicy": object_unresolved_policy(payload),
                        "resolution": object_resolution_json(&resolution),
                    }),
                ),
            ),
            node_result,
        );
    };

    let (response, dropped_edge_ids) =
        session.apply_object_node_replace_current(ApplyObjectNodeReplaceCurrentRequest {
            target,
            node,
            view: payload.view.clone(),
            definition: Some(definition),
            interface_incident_edge_policy: payload.interface_incident_edge_policy,
            mutation: RuntimeMutationRequest {
                graph_patch: None,
                view_patch: None,
                actor_id: Some(identity.client_id.clone()),
                client_id: Some(identity.client_id.clone()),
                description: payload
                    .description
                    .clone()
                    .or_else(|| Some(format!("Realtime graph command {}", frame.message_id))),
            },
        });
    let node_result = node_command_result(
        payload,
        Some(&resolution),
        Some(&node_id),
        dropped_edge_ids,
        None,
    );
    GraphCommandOutcome::with_node_result(response, node_result)
}

fn apply_node_delete_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let node_id = payload.node_id.clone();
    let node_result = node_command_result(payload, None, node_id.as_deref(), Vec::new(), None);
    let target = match validate_object_command_target(session, payload, false) {
        Ok(target) => target,
        Err(response) => return GraphCommandOutcome::with_node_result(*response, node_result),
    };
    let Some(node_id) = node_id else {
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "graph.command.node-id-required",
                    "graph.command kind node.delete requires payload.nodeId",
                    json!({ "commandKind": payload.kind, "target": target }),
                ),
            ),
            node_result,
        );
    };

    let (response, dropped_edge_ids) = session.apply_node_delete_current(
        target,
        node_id.clone(),
        Some(identity.client_id.clone()),
        Some(identity.client_id.clone()),
        payload
            .description
            .clone()
            .or_else(|| Some(format!("Realtime graph command {}", frame.message_id))),
    );
    let node_result = node_command_result(payload, None, Some(&node_id), dropped_edge_ids, None);
    GraphCommandOutcome::with_node_result(response, node_result)
}

fn apply_node_update_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let node_id = payload.node_id.clone();
    let node_result = node_command_result(payload, None, node_id.as_deref(), Vec::new(), None);
    let target = match validate_object_command_target(session, payload, false) {
        Ok(target) => target,
        Err(response) => return GraphCommandOutcome::with_node_result(*response, node_result),
    };
    let Some(node_id) = node_id else {
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "graph.command.node-id-required",
                    "graph.command kind node.update requires payload.nodeId",
                    json!({ "commandKind": payload.kind, "target": target }),
                ),
            ),
            node_result,
        );
    };
    let params = payload.params.clone().unwrap_or_default();
    if params.is_empty() {
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "graph.command.params-required",
                    "graph.command kind node.update requires non-empty payload.params",
                    json!({
                        "commandKind": payload.kind,
                        "target": target,
                        "nodeId": node_id,
                    }),
                ),
            ),
            node_result,
        );
    }

    let response = session.apply_node_update_current(
        target,
        node_id.clone(),
        params,
        Some(identity.client_id.clone()),
        Some(identity.client_id.clone()),
        payload
            .description
            .clone()
            .or_else(|| Some(format!("Realtime graph command {}", frame.message_id))),
    );
    let node_result = node_command_result(payload, None, Some(&node_id), Vec::new(), None);
    GraphCommandOutcome::with_node_result(response, node_result)
}

fn validate_object_command_target(
    session: &crate::RuntimeSession,
    payload: &GraphCommandPayload,
    require_existing: bool,
) -> Result<GraphTargetRef, Box<RuntimePatchResponse>> {
    let Some(target) = payload.target.clone() else {
        return Err(Box::new(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.target-required",
                format!(
                    "graph.command kind {} requires payload.target",
                    payload.kind
                ),
                json!({ "commandKind": payload.kind }),
            ),
        )));
    };
    if let Some(base_graph_revision) = payload.base_graph_revision.as_deref()
        && base_graph_revision != target.base_revision
    {
        return Err(Box::new(graph_command_rejected_response(
            session,
            true,
            RuntimeDiagnostic::structured_error(
                "graph.command.target-revision-conflict",
                format!(
                    "baseGraphRevision {base_graph_revision} does not match target.baseRevision {}",
                    target.base_revision
                ),
                json!({
                    "expectedRevision": base_graph_revision,
                    "actualRevision": target.base_revision,
                    "target": target,
                    "commandKind": payload.kind,
                }),
            ),
        )));
    }
    match session.target_revision_current(&target) {
        Some(actual_revision) if actual_revision != target.base_revision => {
            Err(Box::new(graph_command_rejected_response(
                session,
                true,
                RuntimeDiagnostic::structured_error(
                    "graph.command.target-revision-conflict",
                    format!(
                        "target baseRevision {} does not match target graph revision {}",
                        target.base_revision, actual_revision
                    ),
                    json!({
                        "expectedRevision": target.base_revision,
                        "actualRevision": actual_revision,
                        "target": target,
                        "commandKind": payload.kind,
                    }),
                ),
            )))
        }
        Some(_) => Ok(target),
        None if require_existing => Err(Box::new(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "node.target.missing-graph",
                "node target graph is not available in the active current 0.1 project",
                json!({ "target": target, "commandKind": payload.kind }),
            ),
        ))),
        None => Ok(target),
    }
}

fn generated_node_id_for_create(
    session: &crate::RuntimeSession,
    target: &GraphTargetRef,
    resolution: &ObjectTextResolution,
) -> String {
    let base = node_id_slug(&resolution.display_text)
        .or_else(|| node_id_slug(&resolution.class_symbol))
        .unwrap_or_else(|| "node".to_owned());
    let used = session
        .project_document_current()
        .and_then(|project| graph_for_node_command_target(&project, target).cloned())
        .map(|graph| {
            graph
                .nodes
                .into_iter()
                .map(|node| node.id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    next_generated_node_id(&base, &used)
}

fn graph_for_node_command_target<'a>(
    project: &'a crate::ProjectDocumentCurrent,
    target: &GraphTargetRef,
) -> Option<&'a crate::GraphDocumentCurrent> {
    match &target.path {
        PatchPath::Root => Some(&project.graph),
        PatchPath::HelpWorkingCopy {
            working_copy_id, ..
        } if working_copy_id == &project.graph.id => Some(&project.graph),
        PatchPath::ProjectPatchDefinition { patch_id } => project
            .patch_library
            .iter()
            .find(|patch| patch.id == *patch_id)
            .map(|patch| &patch.graph),
        PatchPath::HelpWorkingCopy { .. }
        | PatchPath::PackagePatchDefinition { .. }
        | PatchPath::EmbeddedPatchInstance { .. } => None,
    }
}

fn node_id_slug(input: &str) -> Option<String> {
    let mut slug = String::new();
    let mut previous_separator = false;
    for character in input.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_separator = false;
        } else if !previous_separator && !slug.is_empty() {
            slug.push('_');
            previous_separator = true;
        }
    }
    while slug.ends_with('_') {
        slug.pop();
    }
    if slug.is_empty() {
        return None;
    }
    if slug
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit())
    {
        slug.insert_str(0, "node_");
    }
    Some(slug)
}

fn next_generated_node_id(base: &str, used: &[String]) -> String {
    if !used.iter().any(|node_id| node_id == base) {
        return base.to_owned();
    }
    for index in 2.. {
        let candidate = format!("{base}_{index}");
        if !used.iter().any(|node_id| node_id == &candidate) {
            return candidate;
        }
    }
    unreachable!("unbounded generated node id loop must return")
}

fn materialize_object_command_node(
    _session: &crate::RuntimeSession,
    payload: &GraphCommandPayload,
    resolution: &ObjectTextResolution,
    node_id: &str,
) -> Option<(crate::GraphNodeCurrent, crate::NodeDefinitionCurrent)> {
    if resolution.ok() {
        let mut node = materialize_object_text_node_v01(resolution, node_id).ok()?;
        merge_payload_params(&mut node.params, payload.params.as_ref());
        let definition = object_text_node_definition_v01(resolution)?;
        return Some((node, definition));
    }
    if object_unresolved_policy(payload) == ObjectUnresolvedPolicy::MaterializeDiagnostic {
        let mut node = materialize_unresolved_object_text_node_v01(resolution, node_id);
        merge_payload_params(&mut node.params, payload.params.as_ref());
        return Some((node, unresolved_object_text_node_definition_v01()));
    }
    None
}

fn merge_payload_params(params: &mut Map<String, Value>, overrides: Option<&Map<String, Value>>) {
    let Some(overrides) = overrides else {
        return;
    };
    for (key, value) in overrides {
        params.insert(key.clone(), value.clone());
    }
}

fn object_unresolved_policy(payload: &GraphCommandPayload) -> ObjectUnresolvedPolicy {
    payload
        .unresolved_policy
        .unwrap_or(ObjectUnresolvedPolicy::MaterializeDiagnostic)
}

fn object_text_runtime_diagnostics(resolution: &ObjectTextResolution) -> Vec<RuntimeDiagnostic> {
    resolution
        .diagnostics
        .iter()
        .map(|diagnostic| {
            RuntimeDiagnostic::structured_error(
                diagnostic.code.clone(),
                diagnostic.message.clone(),
                json!({
                    "surface": "object-text",
                    "objectText": resolution.input,
                    "displayText": resolution.display_text,
                    "classSymbol": resolution.class_symbol,
                    "candidateCount": resolution.candidates.len(),
                    "candidates": resolution.candidates.iter().map(object_text_candidate_json).collect::<Vec<_>>(),
                }),
            )
        })
        .collect()
}

fn node_command_result(
    payload: &GraphCommandPayload,
    resolution: Option<&ObjectTextResolution>,
    node_id: Option<&str>,
    dropped_edge_ids: Vec<String>,
    input: Option<Value>,
) -> Value {
    json!({
        "kind": payload.kind,
        "nodeId": node_id,
        "requestedNodeId": payload.requested_node_id,
        "target": payload.target,
        "objectText": payload.object_text,
        "unresolvedPolicy": object_unresolved_policy(payload),
        "interfaceIncidentEdgePolicy": payload.interface_incident_edge_policy,
        "droppedEdgeIds": dropped_edge_ids,
        "resolution": resolution.map(object_resolution_json),
        "input": input,
    })
}

fn node_input_result(
    request: &RuntimeControlEventRequest,
    response: &RuntimeControlEventResponse,
) -> Value {
    json!({
        "nodeId": request.node_id,
        "portId": request.port_id,
        "message": request.message,
        "accepted": response.ok,
        "changed": response.changed,
        "controlRevision": response.control_revision,
        "emitted": response.emitted,
    })
}

fn object_resolution_json(resolution: &ObjectTextResolution) -> Value {
    json!({
        "input": resolution.input,
        "displayText": resolution.display_text,
        "classSymbol": resolution.class_symbol,
        "resolved": resolution.ok(),
        "resolvedKind": resolution.resolved_kind,
        "resolvedKindVersion": resolution.resolved_kind_version,
        "candidateCount": resolution.candidates.len(),
        "candidates": resolution.candidates.iter().map(object_text_candidate_json).collect::<Vec<_>>(),
        "params": resolution.params,
        "ports": resolution.instance_ports.iter().map(object_text_port_json).collect::<Vec<_>>(),
        "diagnostics": resolution.diagnostics.iter().map(|diagnostic| {
            json!({
                "code": diagnostic.code,
                "message": diagnostic.message,
            })
        }).collect::<Vec<_>>(),
    })
}

fn object_text_candidate_json(candidate: &crate::object_text::ObjectTextCandidateSummary) -> Value {
    json!({
        "id": candidate.id,
        "source": candidate.source,
        "kind": candidate.kind,
        "displayName": candidate.display_name,
    })
}

fn object_text_port_json(port: &crate::object_text::ObjectTextPort) -> Value {
    json!({
        "id": port.id,
        "direction": match &port.direction {
            ObjectTextPortDirection::Input => "input",
            ObjectTextPortDirection::Output => "output",
        },
        "type": port.port_type,
        "rate": match &port.rate {
            ObjectTextPortRate::Event => "event",
            ObjectTextPortRate::Control => "control",
            ObjectTextPortRate::Audio => "audio",
            ObjectTextPortRate::Render => "render",
            ObjectTextPortRate::Gpu => "gpu",
            ObjectTextPortRate::Resource => "resource",
            ObjectTextPortRate::Io => "io",
        },
        "activation": port.activation.as_ref().map(|activation| match activation {
            ObjectTextPortActivation::Trigger => "trigger",
            ObjectTextPortActivation::Latched => "latched",
            ObjectTextPortActivation::Passive => "passive",
        }),
    })
}

fn graph_command_rejected_response(
    session: &crate::RuntimeSession,
    conflict: bool,
    diagnostic: RuntimeDiagnostic,
) -> RuntimePatchResponse {
    RuntimePatchResponse {
        ok: false,
        applied: false,
        conflict,
        snapshot: session.snapshot(),
        history: session.history(),
        diagnostics: vec![diagnostic],
    }
}

fn apply_control_command(
    record: &RuntimeSessionRecord,
    request: RuntimeControlEventRequest,
) -> (
    RuntimeControlEventResponse,
    BTreeMap<String, ControlValue>,
    RuntimeControlEventRequest,
) {
    let (mut response, control_snapshot, changed_values) = {
        let mut session = record
            .session
            .write()
            .expect("runtime session lock should not be poisoned");
        let before = session.control_state_response().values;
        let response = session.apply_control_event(request.clone());
        let after = session.control_state_response().values;
        let changed_values = changed_control_values(&before, &after);
        let control_snapshot = if response.ok && response.changed {
            session.preview_control_state_snapshot()
        } else {
            None
        };
        (response, control_snapshot, changed_values)
    };

    if let Some(control_snapshot) = control_snapshot {
        let mut preview = record
            .preview
            .lock()
            .expect("runtime preview lock should not be poisoned");
        if let Err(error) = preview.update_control_state(control_snapshot) {
            response
                .diagnostics
                .push(RuntimeDiagnostic::warning(format!(
                    "failed to update running preview control state: {error}"
                )));
        }
    }

    (response, changed_values, request)
}

fn changed_control_values(
    before: &BTreeMap<String, ControlValue>,
    after: &BTreeMap<String, ControlValue>,
) -> BTreeMap<String, ControlValue> {
    after
        .iter()
        .filter(|(node_id, value)| before.get(*node_id) != Some(*value))
        .map(|(node_id, value)| (node_id.clone(), value.clone()))
        .collect()
}

fn control_emitted_event(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    request: &RuntimeControlEventRequest,
    response: &mut RuntimeControlEventResponse,
    changed_values: BTreeMap<String, ControlValue>,
    position: RealtimeEventPosition<'_>,
) -> Option<RuntimeRealtimeEnvelope> {
    if response.emitted.is_empty() && changed_values.is_empty() {
        return None;
    }

    Some(RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: EVENT_CONTROL_EMITTED.to_owned(),
        message_id: format!("{}_control_{:06}", record.id, position.sequence),
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
        sequence: Some(position.sequence),
        cursor: Some(position.cursor.to_owned()),
        created_at: Some(created_at_now()),
        payload: json!({
            "commandId": frame.command_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "correlationId": frame.correlation_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "idempotencyKey": frame.idempotency_key,
            "controlSequence": position.sequence,
            "controlRevision": response.control_revision,
            "changed": response.changed,
            "request": request,
            "emitted": response.emitted,
            "values": changed_values,
            "diagnostics": response.diagnostics,
            "replayed": false,
        }),
    })
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

fn graph_ack(context: &GraphEventContext<'_>, cached: bool) -> RuntimeRealtimeEnvelope {
    graph_ack_with_payload(
        context.record,
        context.identity,
        context.frame,
        json!({
            "status": if context.response.ok { "accepted" } else if context.response.conflict { "conflict" } else { "rejected" },
            "accepted": context.response.ok,
            "applied": context.response.applied,
            "conflict": context.response.conflict,
            "cached": cached,
            "graphSequence": context.position.sequence,
            "commandId": context.frame.command_id.clone().unwrap_or_else(|| context.frame.message_id.clone()),
            "correlationId": context.frame.correlation_id.clone().unwrap_or_else(|| context.frame.message_id.clone()),
            "idempotencyKey": context.frame.idempotency_key,
            "eventCursor": context.position.cursor,
            "kind": context.command.kind,
            "target": context.command.target,
            "surfacePath": context.command.surface_path,
            "baseSessionRevision": context.command.base_session_revision,
            "baseGraphRevision": context.command.base_graph_revision,
            "baseViewRevision": context.command.base_view_revision.or_else(|| context.command.view_patch.as_ref().map(|patch| patch.base_view_revision)),
            "node": context.node_result,
            "operation": context.operation_result,
            "sessionRevision": context.response.snapshot.session_revision,
            "graphRevision": context.response.snapshot.graph_revision(),
            "viewRevision": context.response.snapshot.view_revision,
            "historySummary": {
                "latestEntryId": context.response.history.entries.last().map(|entry| entry.id.clone()),
                "canUndo": context.response.history.can_undo,
                "canRedo": context.response.history.can_redo,
                "undoDepth": context.response.history.undo_depth,
                "redoDepth": context.response.history.redo_depth,
            },
            "diagnostics": context.response.diagnostics,
        }),
    )
}

fn graph_ack_from_cached(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    cached: RuntimeRealtimeCachedCommandResult,
) -> RuntimeRealtimeEnvelope {
    let mut payload = mark_ack_payload_cached(cached.ack_payload);
    if let Some(object) = payload.as_object_mut() {
        object.insert("eventCursor".to_owned(), Value::String(cached.event_cursor));
    }
    graph_ack_with_payload(record, identity, frame, payload)
}

fn graph_applied_event(context: &GraphEventContext<'_>) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: EVENT_GRAPH_APPLIED.to_owned(),
        message_id: format!(
            "{}_graph_{:06}",
            context.record.id, context.position.sequence
        ),
        session_id: context.record.id.clone(),
        connection_id: Some(context.identity.connection_id.clone()),
        client_id: Some(context.identity.client_id.clone()),
        window_id: Some(context.identity.window_id.clone()),
        command_id: context
            .frame
            .command_id
            .clone()
            .or_else(|| Some(context.frame.message_id.clone())),
        correlation_id: context
            .frame
            .correlation_id
            .clone()
            .or_else(|| Some(context.frame.message_id.clone())),
        idempotency_key: context.frame.idempotency_key.clone(),
        sequence: Some(context.position.sequence),
        cursor: Some(context.position.cursor.to_owned()),
        created_at: Some(created_at_now()),
        payload: json!({
            "commandId": context.frame.command_id.clone().unwrap_or_else(|| context.frame.message_id.clone()),
            "correlationId": context.frame.correlation_id.clone().unwrap_or_else(|| context.frame.message_id.clone()),
            "idempotencyKey": context.frame.idempotency_key,
            "graphSequence": context.position.sequence,
            "kind": context.command.kind,
            "target": context.command.target,
            "surfacePath": context.command.surface_path,
            "baseSessionRevision": context.command.base_session_revision,
            "baseGraphRevision": context.command.base_graph_revision,
            "baseViewRevision": context.command.base_view_revision.or_else(|| context.command.view_patch.as_ref().map(|patch| patch.base_view_revision)),
            "node": context.node_result,
            "operation": context.operation_result,
            "sessionRevision": context.response.snapshot.session_revision,
            "graphRevision": context.response.snapshot.graph_revision(),
            "viewRevision": context.response.snapshot.view_revision,
            "historyEntryId": context.response.history.entries.last().map(|entry| entry.id.clone()),
            "diagnostics": context.response.diagnostics,
            "replayed": false,
        }),
    }
}

fn node_catalog_changed_event(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    snapshot: NodeCatalogSnapshotV01,
    sequence: u64,
    cursor: String,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: EVENT_NODE_CATALOG_CHANGED.to_owned(),
        message_id: format!("{}_node_catalog_changed_{sequence:06}", record.id),
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
        sequence: Some(sequence),
        cursor: Some(cursor),
        created_at: Some(created_at_now()),
        payload: json!({
            "catalogRevision": snapshot.catalog_revision.clone(),
            "snapshot": snapshot,
            "replayed": false,
        }),
    }
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

fn node_catalog_response(
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
        message_id: format!("{}_node_catalog_{}", record.id, frame.message_id),
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

fn hello_node_catalog_payload(
    record: &RuntimeSessionRecord,
    request: Option<&NodeCatalogHelloRequest>,
) -> Value {
    let snapshot = node_catalog_snapshot_for_record(record);
    match request.map(|request| request.mode).unwrap_or_default() {
        NodeCatalogHelloMode::None => node_catalog_status_payload("notRequested", snapshot, false),
        NodeCatalogHelloMode::IfChanged
            if catalog_revision_matches(
                request.and_then(|request| request.known_revision.as_ref()),
                &snapshot.catalog_revision,
            ) =>
        {
            node_catalog_status_payload("unchanged", snapshot, false)
        }
        NodeCatalogHelloMode::IfChanged | NodeCatalogHelloMode::Always => {
            node_catalog_status_payload("included", snapshot, true)
        }
    }
}

fn node_catalog_status_payload(
    status: &str,
    snapshot: NodeCatalogSnapshotV01,
    include_snapshot: bool,
) -> Value {
    let mut payload = json!({
        "status": status,
        "catalogRevision": snapshot.catalog_revision.clone(),
    });
    if include_snapshot && let Some(object) = payload.as_object_mut() {
        object.insert(
            "snapshot".to_owned(),
            serde_json::to_value(snapshot).expect("node catalog snapshot should serialize"),
        );
    }
    payload
}

fn catalog_revision_matches(
    known_revision: Option<&Value>,
    catalog_revision: &PackageChecksumV01,
) -> bool {
    let Some(known_revision) = known_revision else {
        return false;
    };
    if known_revision.as_str() == Some(catalog_revision.value.as_str()) {
        return true;
    }
    serde_json::to_value(catalog_revision).expect("node catalog revision should serialize")
        == *known_revision
}

fn node_catalog_snapshot_response_payload(snapshot: NodeCatalogSnapshotV01) -> Value {
    node_catalog_status_payload("included", snapshot, true)
}

fn node_catalog_unchanged_response_payload(snapshot: NodeCatalogSnapshotV01) -> Value {
    node_catalog_status_payload("unchanged", snapshot, false)
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

pub(crate) fn node_catalog_snapshot_for_record(
    record: &RuntimeSessionRecord,
) -> NodeCatalogSnapshotV01 {
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    node_catalog_snapshot_for_session(&session)
}

fn node_catalog_snapshot_for_session(session: &crate::RuntimeSession) -> NodeCatalogSnapshotV01 {
    session.node_catalog_snapshot()
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
mod tests {
    use super::*;

    fn test_binding_format() -> EndpointBindingValueFormat {
        EndpointBindingValueFormat {
            binding_id: "edge_value_target".to_owned(),
            binding_epoch: 2,
            format_revision: 7,
            format_digest: None,
            value_format: crate::ValueFormat {
                value_type_id: "value.core.float32".to_owned(),
                format: Some("f32".to_owned()),
                shape: None,
                dynamic_shape: None,
                layout: None,
                strides: None,
                byte_length: None,
                sample_rate: None,
                channels: None,
                channel_layout: None,
                color_space: None,
                color_range: None,
                transfer: None,
                primaries: None,
                alpha_policy: None,
                resource_kind: None,
            },
            source: Some(crate::ValueEndpointRef {
                node_id: "value_1".to_owned(),
                port_id: "value".to_owned(),
            }),
            target: Some(crate::ValueEndpointRef {
                node_id: "target_1".to_owned(),
                port_id: "cold".to_owned(),
            }),
            delivery: None,
        }
    }

    fn test_occurrence_header() -> ValueOccurrenceHeader {
        ValueOccurrenceHeader {
            binding_id: "edge_value_target".to_owned(),
            binding_epoch: 2,
            format_revision: 7,
            sequence: 1,
            clock: None,
            timestamp: None,
            payload_kind: crate::ValuePayloadKind::Json,
            byte_length: None,
            byte_offset: None,
            actual_shape: None,
            flags: None,
            dropped_before: None,
            duration: None,
        }
    }

    fn realtime_event(session_id: &str, sequence: u64, cursor: &str) -> RuntimeRealtimeEnvelope {
        RuntimeRealtimeEnvelope {
            schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
            schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
            message_type: EVENT_PRESENCE_UPDATED.to_owned(),
            message_id: format!("{session_id}_presence_{sequence:06}"),
            session_id: session_id.to_owned(),
            connection_id: None,
            client_id: None,
            window_id: None,
            command_id: None,
            correlation_id: None,
            idempotency_key: None,
            sequence: Some(sequence),
            cursor: Some(cursor.to_owned()),
            created_at: Some(created_at_now()),
            payload: json!({ "replayed": false }),
        }
    }

    fn client_frame(
        session_id: &str,
        message_type: &str,
        message_id: &str,
        payload: Value,
    ) -> RuntimeRealtimeEnvelope {
        RuntimeRealtimeEnvelope {
            schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
            schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
            message_type: message_type.to_owned(),
            message_id: message_id.to_owned(),
            session_id: session_id.to_owned(),
            connection_id: None,
            client_id: None,
            window_id: None,
            command_id: Some(format!("{message_id}-command")),
            correlation_id: Some(format!("{message_id}-correlation")),
            idempotency_key: None,
            sequence: None,
            cursor: None,
            created_at: None,
            payload,
        }
    }

    fn graph_payload(payload: Value) -> GraphCommandPayload {
        serde_json::from_value(payload).expect("graph command payload should parse")
    }

    fn root_target(base_revision: &str) -> GraphTargetRef {
        GraphTargetRef {
            path: PatchPath::Root,
            base_revision: base_revision.to_owned(),
            target_revision: None,
        }
    }

    fn package_patch_target(base_revision: &str) -> GraphTargetRef {
        GraphTargetRef {
            path: PatchPath::PackagePatchDefinition {
                package_id: "pkg".to_owned(),
                patch_id: "help".to_owned(),
                version: None,
            },
            base_revision: base_revision.to_owned(),
            target_revision: None,
        }
    }

    fn test_identity() -> RuntimeRealtimeConnectionIdentity {
        RuntimeRealtimeConnectionIdentity {
            connection_id: "conn_test".to_owned(),
            client_id: "client_test".to_owned(),
            window_id: "window_test".to_owned(),
            resume_token: "rtresume-test".to_owned(),
        }
    }

    fn port_for_json(
        id: &str,
        direction: ObjectTextPortDirection,
        rate: ObjectTextPortRate,
        activation: Option<ObjectTextPortActivation>,
    ) -> crate::object_text::ObjectTextPort {
        crate::object_text::ObjectTextPort {
            id: id.to_owned(),
            direction,
            port_type: "value.core.float32".to_owned(),
            label: Some(format!("{id} label")),
            rate,
            accepts: None,
            activation,
            message_keys: None,
        }
    }

    fn resolved_float_object_text(input: &str) -> ObjectTextResolution {
        ObjectRegistry::first_party_core().resolve(input)
    }

    fn object_text_resolution_with_all_port_variants() -> ObjectTextResolution {
        let mut resolution = resolved_float_object_text("sig~ 440");
        resolution.instance_ports = vec![
            port_for_json(
                "event_in",
                ObjectTextPortDirection::Input,
                ObjectTextPortRate::Event,
                Some(ObjectTextPortActivation::Trigger),
            ),
            port_for_json(
                "control_in",
                ObjectTextPortDirection::Input,
                ObjectTextPortRate::Control,
                Some(ObjectTextPortActivation::Latched),
            ),
            port_for_json(
                "audio_out",
                ObjectTextPortDirection::Output,
                ObjectTextPortRate::Audio,
                Some(ObjectTextPortActivation::Passive),
            ),
            port_for_json(
                "render_out",
                ObjectTextPortDirection::Output,
                ObjectTextPortRate::Render,
                None,
            ),
            port_for_json(
                "gpu_out",
                ObjectTextPortDirection::Output,
                ObjectTextPortRate::Gpu,
                None,
            ),
            port_for_json(
                "resource_out",
                ObjectTextPortDirection::Output,
                ObjectTextPortRate::Resource,
                None,
            ),
            port_for_json(
                "io_out",
                ObjectTextPortDirection::Output,
                ObjectTextPortRate::Io,
                None,
            ),
        ];
        resolution.candidates = vec![crate::object_text::ObjectTextCandidateSummary {
            id: "object.core.sig".to_owned(),
            source: "core".to_owned(),
            kind: "object.core.sig".to_owned(),
            display_name: "Signal".to_owned(),
        }];
        resolution
            .diagnostics
            .push(crate::object_text::ObjectTextDiagnostic {
                code: "object-text.test-warning".to_owned(),
                message: "test warning".to_owned(),
            });
        resolution
    }

    fn assert_response_diagnostic_code(response: &RuntimePatchResponse, code: &str) {
        assert_eq!(response.diagnostics[0].code.as_deref(), Some(code));
    }

    #[test]
    fn value_occurrence_header_guard_accepts_current_binding() {
        let binding = test_binding_format();
        let header = test_occurrence_header();
        let binding_formats = [binding.clone()];

        let accepted =
            validate_value_occurrence_header_for_session_binding(&header, &binding_formats)
                .expect("current binding should be accepted");

        assert_eq!(accepted, &binding);
    }

    #[test]
    fn value_occurrence_header_guard_rejects_invalid_header() {
        let mut header = test_occurrence_header();
        header.binding_id.clear();

        let diagnostic =
            validate_value_occurrence_header_for_session_binding(&header, &[test_binding_format()])
                .expect_err("invalid header should be rejected");

        assert_eq!(
            diagnostic.code.as_deref(),
            Some("runtime.value-binding.invalid-header")
        );
    }

    #[test]
    fn value_occurrence_header_guard_rejects_unknown_binding() {
        let mut header = test_occurrence_header();
        header.binding_id = "missing_edge".to_owned();

        let diagnostic =
            validate_value_occurrence_header_for_session_binding(&header, &[test_binding_format()])
                .expect_err("unknown binding should be rejected");

        assert_eq!(
            diagnostic.code.as_deref(),
            Some("runtime.value-binding.unknown-binding")
        );
    }

    #[test]
    fn value_occurrence_header_guard_rejects_stale_binding_metadata() {
        let binding = test_binding_format();
        let mut stale_epoch = test_occurrence_header();
        stale_epoch.binding_epoch = 1;
        let epoch_diagnostic = validate_value_occurrence_header_for_session_binding(
            &stale_epoch,
            std::slice::from_ref(&binding),
        )
        .expect_err("stale epoch should be rejected");
        assert_eq!(
            epoch_diagnostic.code.as_deref(),
            Some("runtime.value-binding.stale-epoch")
        );

        let mut stale_format = test_occurrence_header();
        stale_format.format_revision = 6;
        let format_diagnostic =
            validate_value_occurrence_header_for_session_binding(&stale_format, &[binding])
                .expect_err("stale format revision should be rejected");
        assert_eq!(
            format_diagnostic.code.as_deref(),
            Some("runtime.value-binding.stale-format-revision")
        );
    }

    #[test]
    fn idempotency_results_follow_retained_event_window() {
        let state = RuntimeRealtimeState::new("default", 2);
        let identity = state.issue_connection_identity(None);

        for sequence in 1..=3 {
            let cursor = state.cursor_for(sequence);
            let idempotency_key = format!("key-{sequence}");
            state.remember_ack(RememberAckInput {
                identity: &identity,
                message_type: "presence.update",
                idempotency_key: &idempotency_key,
                event_cursor: &cursor,
                event_sequence: sequence,
                ack_payload: json!({ "eventCursor": cursor }),
                emitted_results: Vec::new(),
            });
            state.publish(realtime_event("default", sequence, &cursor));
        }

        let idempotency_results = state
            .idempotency_results
            .lock()
            .expect("runtime realtime idempotency lock should not be poisoned");
        assert_eq!(idempotency_results.len(), 2);
        assert!(
            !idempotency_results.contains_key(&RuntimeRealtimeIdempotencyScope {
                client_id: identity.client_id.clone(),
                window_id: identity.window_id.clone(),
                message_type: "presence.update".to_owned(),
                idempotency_key: "key-1".to_owned(),
            })
        );
        assert!(
            idempotency_results.contains_key(&RuntimeRealtimeIdempotencyScope {
                client_id: identity.client_id.clone(),
                window_id: identity.window_id.clone(),
                message_type: "presence.update".to_owned(),
                idempotency_key: "key-2".to_owned(),
            })
        );
        assert!(
            idempotency_results.contains_key(&RuntimeRealtimeIdempotencyScope {
                client_id: identity.client_id.clone(),
                window_id: identity.window_id.clone(),
                message_type: "presence.update".to_owned(),
                idempotency_key: "key-3".to_owned(),
            })
        );
    }

    #[test]
    fn replay_after_reports_cursor_diagnostics_and_marks_replayed_events() {
        let state = RuntimeRealtimeState::new("default", 2);
        let current_cursor = state.current_cursor();
        let (incarnation_id, _) = current_cursor
            .rsplit_once(':')
            .expect("runtime cursor should include sequence separator");

        let invalid_shape = state
            .replay_after("not-a-runtime-cursor")
            .expect_err("cursor without sequence separator should be rejected");
        assert_eq!(invalid_shape.code, "realtime.cursor.invalid");

        let wrong_incarnation = state
            .replay_after("other-incarnation:0")
            .expect_err("cursor from another incarnation should be rejected");
        assert_eq!(
            wrong_incarnation.code,
            "realtime.cursor.incarnation-mismatch"
        );

        let invalid_sequence = state
            .replay_after(&format!("{incarnation_id}:not-a-number"))
            .expect_err("cursor with non-numeric sequence should be rejected");
        assert_eq!(invalid_sequence.code, "realtime.cursor.invalid");

        let ahead = state
            .replay_after(&format!("{incarnation_id}:1"))
            .expect_err("cursor ahead of the current sequence should require sync");
        assert_eq!(ahead.code, "realtime.cursor.unknown");

        for expected_sequence in 1..=3 {
            let sequence = state.next_event_sequence();
            assert_eq!(sequence, expected_sequence);
            let cursor = state.cursor_for(sequence);
            state.publish(realtime_event("default", sequence, &cursor));
        }

        let expired = state
            .replay_after(&state.cursor_for(0))
            .expect_err("cursor before retained window should require sync");
        assert_eq!(expired.code, "realtime.cursor.expired");

        let replay = state
            .replay_after(&state.cursor_for(2))
            .expect("cursor inside retained window should replay later events");
        assert_eq!(replay.high_water_sequence, 3);
        assert_eq!(replay.events.len(), 1);
        assert_eq!(replay.events[0].sequence, Some(3));
        assert_eq!(replay.events[0].payload["replayed"], true);
    }

    #[test]
    fn presence_entries_are_ttl_pruned_and_count_bounded() {
        let state = RuntimeRealtimeState::new("default", 1);
        let now = SystemTime::now();
        let expired_at = now
            .checked_sub(Duration::from_secs(1))
            .expect("test time should support subtraction");
        let future = now + Duration::from_secs(60);
        let expired = state.issue_connection_identity(None);
        let active_a = state.issue_connection_identity(None);
        let active_b = state.issue_connection_identity(None);
        let active_c = state.issue_connection_identity(None);

        state.remember_presence(&expired, json!({ "client": "expired" }), expired_at, 1);
        state.remember_presence(&active_a, json!({ "client": "a" }), future, 2);
        state.remember_presence(&active_b, json!({ "client": "b" }), future, 3);
        state.remember_presence(&active_c, json!({ "client": "c" }), future, 4);

        let presence = state
            .presence
            .lock()
            .expect("runtime realtime presence lock should not be poisoned");
        assert_eq!(presence.len(), 2);
        assert!(!presence.contains_key(&format!("{}:{}", expired.client_id, expired.window_id)));
        assert!(!presence.contains_key(&format!("{}:{}", active_a.client_id, active_a.window_id)));
        assert!(presence.contains_key(&format!("{}:{}", active_b.client_id, active_b.window_id)));
        assert!(presence.contains_key(&format!("{}:{}", active_c.client_id, active_c.window_id)));
    }

    #[test]
    fn hello_catalog_and_error_envelopes_preserve_client_context() {
        let registry = crate::RuntimeSessionRegistry::dry_preview();
        let record = registry.default_record();
        let identity =
            record
                .realtime
                .issue_connection_identity(Some(RuntimeRealtimeResumeIdentity {
                    client_id: "resumed-client".to_owned(),
                    window_id: "resumed-window".to_owned(),
                    expires_at: SystemTime::now() + Duration::from_secs(60),
                }));
        let mut frame = client_frame(
            &record.id,
            "session.hello",
            "hello-1",
            json!({ "nodeCatalog": { "mode": "always" } }),
        );
        frame.cursor = Some("cursor-from-frame".to_owned());

        let decoded = decode_hello_payload(&frame);
        assert_eq!(decoded.last_cursor.as_deref(), Some("cursor-from-frame"));
        assert_eq!(
            decoded.node_catalog.as_ref().map(|request| request.mode),
            Some(NodeCatalogHelloMode::Always)
        );

        let snapshot = current_snapshot(&record);
        let catalog_snapshot = node_catalog_snapshot_for_record(&record);
        let current_revision = serde_json::to_value(&catalog_snapshot.catalog_revision)
            .expect("catalog revision should serialize");
        let included_catalog = hello_node_catalog_payload(&record, decoded.node_catalog.as_ref());
        assert_eq!(included_catalog["status"], "included");
        assert!(included_catalog.get("snapshot").is_some());
        let unchanged_catalog = hello_node_catalog_payload(
            &record,
            Some(&NodeCatalogHelloRequest {
                mode: NodeCatalogHelloMode::IfChanged,
                known_revision: Some(current_revision.clone()),
            }),
        );
        assert_eq!(unchanged_catalog["status"], "unchanged");
        assert!(unchanged_catalog.get("snapshot").is_none());
        assert!(catalog_revision_matches(
            Some(&Value::String(
                catalog_snapshot.catalog_revision.value.clone()
            )),
            &catalog_snapshot.catalog_revision,
        ));
        assert!(catalog_revision_matches(
            Some(&current_revision),
            &catalog_snapshot.catalog_revision,
        ));
        assert!(!catalog_revision_matches(
            None,
            &catalog_snapshot.catalog_revision
        ));

        let attached = session_attached(
            &record,
            &identity,
            &frame,
            &snapshot,
            decoded.node_catalog.as_ref(),
        );
        assert_eq!(attached.message_type, "session.attached");
        assert_eq!(attached.correlation_id.as_deref(), Some("hello-1"));
        assert_eq!(attached.payload["clientId"], identity.client_id);
        assert_eq!(attached.payload["nodeCatalog"]["status"], "included");
        assert_eq!(
            attached.payload["currentRevisions"]["sessionRevision"],
            snapshot.session_revision
        );

        let sync = session_sync_required(
            &record,
            &identity,
            &frame,
            &snapshot,
            sync_required_diagnostic(
                "realtime.cursor.test",
                "test sync required",
                Some(json!({ "currentCursor": record.realtime.current_cursor() })),
            ),
            Some(&NodeCatalogHelloRequest {
                mode: NodeCatalogHelloMode::IfChanged,
                known_revision: Some(current_revision),
            }),
        );
        assert_eq!(sync.message_type, "session.syncRequired");
        assert_eq!(sync.payload["diagnostic"]["code"], "realtime.cursor.test");
        assert_eq!(sync.payload["nodeCatalog"]["status"], "unchanged");

        let error = runtime_error(
            &record.id,
            Some(&identity),
            Some(&frame),
            "realtime.frame.test",
            "test error",
            Some(json!({ "field": "payload" })),
        );
        assert_eq!(error.message_type, "runtime.error");
        assert_eq!(
            error.connection_id.as_deref(),
            Some(identity.connection_id.as_str())
        );
        assert_eq!(error.command_id.as_deref(), Some("hello-1-command"));
        assert_eq!(error.correlation_id.as_deref(), Some("hello-1"));
        assert_eq!(error.payload["diagnostic"]["details"]["field"], "payload");

        let internal = empty_correlation_frame(&record.id);
        assert_eq!(internal.message_type, "runtime.internal");
        assert_eq!(internal.session_id, record.id);
        assert_eq!(internal.payload, Value::Null);
    }

    #[test]
    fn command_handlers_reject_invalid_payloads_and_clamp_presence_ttl() {
        let registry = crate::RuntimeSessionRegistry::dry_preview();
        let record = registry.default_record();
        let identity = record.realtime.issue_connection_identity(None);

        let missing_presence_key = handle_presence_update(
            &record,
            &identity,
            client_frame(
                &record.id,
                "presence.update",
                "presence-missing-key",
                json!({ "presence": { "tool": "select" } }),
            ),
        )
        .expect_err("presence updates require idempotency keys");
        assert_eq!(
            missing_presence_key.code,
            "realtime.command.idempotency-key-required"
        );

        let mut invalid_presence = client_frame(
            &record.id,
            "presence.update",
            "presence-invalid",
            Value::String("bad-presence".to_owned()),
        );
        invalid_presence.idempotency_key = Some("presence-invalid-key".to_owned());
        let invalid_presence = handle_presence_update(&record, &identity, invalid_presence)
            .expect_err("non-object presence payload should be rejected");
        assert_eq!(invalid_presence.code, "realtime.presence.invalid-payload");

        let mut valid_presence = client_frame(
            &record.id,
            "presence.update",
            "presence-valid",
            json!({ "presence": { "tool": "draw" }, "ttlMs": 1 }),
        );
        valid_presence.idempotency_key = Some("presence-valid-key".to_owned());
        let (ack, event) = handle_presence_update(&record, &identity, valid_presence)
            .expect("valid presence update should ack and publish");
        let event = event.expect("presence update should create a realtime event");
        assert_eq!(ack.message_type, "command.ack");
        assert_eq!(ack.payload["accepted"], true);
        assert_eq!(event.message_type, "presence.updated");
        assert_eq!(event.payload["presence"]["presence"]["tool"], "draw");
        assert_eq!(event.payload["presence"]["ttlMs"], 1000);

        let graph_missing_key = handle_graph_command(
            &record,
            &identity,
            client_frame(
                &record.id,
                "graph.command",
                "graph-missing-key",
                json!({ "kind": "node.resolve" }),
            ),
        )
        .expect_err("graph commands require idempotency keys");
        assert_eq!(
            graph_missing_key.code,
            "realtime.command.idempotency-key-required"
        );

        let mut invalid_graph = client_frame(
            &record.id,
            "graph.command",
            "graph-invalid",
            json!({ "baseSessionRevision": 0 }),
        );
        invalid_graph.idempotency_key = Some("graph-invalid-key".to_owned());
        let invalid_graph = handle_graph_command(&record, &identity, invalid_graph)
            .expect_err("missing graph kind should be rejected by payload decoding");
        assert_eq!(invalid_graph.code, "realtime.graph.invalid-payload");
    }

    #[test]
    fn node_command_result_serializes_resolution_ports_diagnostics_and_input() {
        let payload = graph_payload(json!({
            "kind": "node.replace",
            "target": root_target("1"),
            "objectText": "sig~ 440",
            "nodeId": "oscillator",
            "requestedNodeId": "requested-oscillator",
            "unresolvedPolicy": "reject",
            "interfaceIncidentEdgePolicy": "reject",
            "surfacePath": ["root", "oscillator"]
        }));
        let resolution = object_text_resolution_with_all_port_variants();

        let node_result = node_command_result(
            &payload,
            Some(&resolution),
            Some("oscillator"),
            vec!["stale-edge".to_owned()],
            Some(json!({ "message": ControlMessage::bang() })),
        );

        assert_eq!(node_result["kind"], "node.replace");
        assert_eq!(node_result["nodeId"], "oscillator");
        assert_eq!(node_result["requestedNodeId"], "requested-oscillator");
        assert_eq!(node_result["interfaceIncidentEdgePolicy"], "reject");
        assert_eq!(node_result["droppedEdgeIds"], json!(["stale-edge"]));
        let rates = node_result["resolution"]["ports"]
            .as_array()
            .expect("ports should serialize")
            .iter()
            .map(|port| port["rate"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(
            rates,
            vec![
                "event", "control", "audio", "render", "gpu", "resource", "io"
            ]
        );
        let activations = node_result["resolution"]["ports"]
            .as_array()
            .expect("ports should serialize")
            .iter()
            .map(|port| port["activation"].clone())
            .collect::<Vec<_>>();
        assert_eq!(
            activations[..3],
            [
                Value::String("trigger".to_owned()),
                Value::String("latched".to_owned()),
                Value::String("passive".to_owned())
            ]
        );
        assert_eq!(
            node_result["resolution"]["diagnostics"][0]["code"],
            "object-text.test-warning"
        );

        let diagnostics = object_text_runtime_diagnostics(&resolution);
        assert_eq!(
            diagnostics[0].code.as_deref(),
            Some("object-text.test-warning")
        );
        assert_eq!(
            diagnostics[0]
                .details
                .as_ref()
                .expect("object text diagnostics should include structured details")["candidateCount"],
            1
        );

        let input_result = node_input_result(
            &RuntimeControlEventRequest {
                node_id: "oscillator".to_owned(),
                port_id: "frequency".to_owned(),
                message: ControlMessage::from_value(ControlValue::float(440.0)),
            },
            &RuntimeControlEventResponse {
                ok: true,
                changed: true,
                control_revision: Some(7),
                emitted: Vec::new(),
                diagnostics: Vec::new(),
            },
        );
        assert_eq!(input_result["accepted"], true);
        assert_eq!(input_result["controlRevision"], 7);
    }

    #[test]
    fn object_command_materialization_respects_params_and_unresolved_policy() {
        let session = crate::RuntimeSession::default();
        let payload = graph_payload(json!({
            "kind": "node.create",
            "target": root_target("1"),
            "objectText": "float 1",
            "params": { "frequency": 880.0 }
        }));
        let materialized = materialize_object_command_node(
            &session,
            &payload,
            &resolved_float_object_text("float 1"),
            "float_1",
        )
        .expect("resolved object text should materialize");
        assert_eq!(materialized.0.params["frequency"], 880.0);

        let unresolved = ObjectRegistry::first_party_core().resolve("missingObject 1");
        let diagnostic_payload = graph_payload(json!({
            "kind": "node.create",
            "target": root_target("1"),
            "objectText": "missingObject 1",
            "params": { "label": "keep me" }
        }));
        let diagnostic_node = materialize_object_command_node(
            &session,
            &diagnostic_payload,
            &unresolved,
            "missing_1",
        )
        .expect("default unresolved policy should materialize a diagnostic node");
        assert_eq!(diagnostic_node.0.id, "missing_1");
        assert_eq!(diagnostic_node.0.params["label"], "keep me");

        let reject_payload = graph_payload(json!({
            "kind": "node.create",
            "target": root_target("1"),
            "objectText": "missingObject 1",
            "unresolvedPolicy": "reject"
        }));
        assert!(
            materialize_object_command_node(&session, &reject_payload, &unresolved, "missing_2")
                .is_none()
        );
    }

    #[test]
    fn object_command_helpers_validate_required_fields_and_targets() {
        let identity = test_identity();
        let frame = client_frame("default", "graph.command", "graph-helpers", Value::Null);
        let mut session = crate::RuntimeSession::default();

        let missing_resolve = apply_object_resolve_graph_command(
            &session,
            &graph_payload(json!({ "kind": "node.resolve" })),
        );
        assert_response_diagnostic_code(
            &missing_resolve.response,
            "graph.command.object-text-required",
        );
        let missing_create = apply_object_create_graph_command(
            &mut session,
            &identity,
            &frame,
            &graph_payload(json!({ "kind": "node.create" })),
        );
        assert_response_diagnostic_code(
            &missing_create.response,
            "graph.command.object-text-required",
        );
        let missing_replace = apply_object_replace_graph_command(
            &mut session,
            &identity,
            &frame,
            &graph_payload(json!({ "kind": "node.replace" })),
        );
        assert_response_diagnostic_code(
            &missing_replace.response,
            "graph.command.object-text-required",
        );
        let missing_delete_node = apply_node_delete_graph_command(
            &mut session,
            &identity,
            &frame,
            &graph_payload(json!({ "kind": "node.delete", "target": root_target("1") })),
        );
        assert_response_diagnostic_code(
            &missing_delete_node.response,
            "graph.command.node-id-required",
        );
        let missing_update_node = apply_node_update_graph_command(
            &mut session,
            &identity,
            &frame,
            &graph_payload(json!({ "kind": "node.update", "target": root_target("1") })),
        );
        assert_response_diagnostic_code(
            &missing_update_node.response,
            "graph.command.node-id-required",
        );
        let empty_update = apply_node_update_graph_command(
            &mut session,
            &identity,
            &frame,
            &graph_payload(json!({
                "kind": "node.update",
                "target": root_target("1"),
                "nodeId": "value_1"
            })),
        );
        assert_response_diagnostic_code(&empty_update.response, "graph.command.params-required");

        let no_target = validate_object_command_target(
            &session,
            &graph_payload(json!({ "kind": "node.resolve" })),
            true,
        )
        .expect_err("node commands require a target");
        assert_response_diagnostic_code(&no_target, "graph.command.target-required");
        let revision_conflict = validate_object_command_target(
            &session,
            &graph_payload(json!({
                "kind": "node.create",
                "target": root_target("1"),
                "baseGraphRevision": "2"
            })),
            false,
        )
        .expect_err("baseGraphRevision must agree with target.baseRevision");
        assert!(revision_conflict.conflict);
        assert_response_diagnostic_code(
            &revision_conflict,
            "graph.command.target-revision-conflict",
        );
        let missing_graph = validate_object_command_target(
            &session,
            &graph_payload(json!({ "kind": "node.resolve", "target": root_target("1") })),
            true,
        )
        .expect_err("node.resolve requires an existing target graph");
        assert_response_diagnostic_code(&missing_graph, "node.target.missing-graph");
    }

    #[test]
    fn node_id_generation_helpers_are_stable_for_object_text_commands() {
        assert_eq!(
            node_id_slug("123 Weird Object!!"),
            Some("node_123_weird_object".to_owned())
        );
        assert_eq!(node_id_slug("   "), None);
        assert_eq!(
            next_generated_node_id(
                "osc",
                &["osc".to_owned(), "osc_2".to_owned(), "other".to_owned()]
            ),
            "osc_3"
        );
    }

    #[test]
    fn graph_command_validation_covers_view_and_change_set_rejections() {
        let registry = crate::RuntimeSessionRegistry::dry_preview();
        let record = registry.default_record();
        let identity = test_identity();
        let frame = client_frame(&record.id, "graph.command", "graph-validate", Value::Null);

        let session_conflict = apply_graph_command(
            &record,
            &identity,
            &frame,
            &graph_payload(json!({ "kind": "node.resolve", "baseSessionRevision": 99 })),
        );
        assert!(session_conflict.response.conflict);
        assert_response_diagnostic_code(
            &session_conflict.response,
            "graph.command.session-revision-conflict",
        );

        let missing_view_patch = apply_graph_command(
            &record,
            &identity,
            &frame,
            &graph_payload(json!({ "kind": "view.patch" })),
        );
        assert_response_diagnostic_code(
            &missing_view_patch.response,
            "graph.command.view-patch-required",
        );

        let view_revision_conflict = apply_graph_command(
            &record,
            &identity,
            &frame,
            &graph_payload(json!({
                "kind": "view.patch",
                "baseViewRevision": 2,
                "viewPatch": { "baseViewRevision": 1, "ops": [] }
            })),
        );
        assert!(view_revision_conflict.response.conflict);
        assert_response_diagnostic_code(
            &view_revision_conflict.response,
            "graph.command.view-revision-conflict",
        );

        let graph_revision_conflict = apply_graph_command(
            &record,
            &identity,
            &frame,
            &graph_payload(json!({
                "kind": "view.patch",
                "baseGraphRevision": "1",
                "viewPatch": { "baseViewRevision": 0, "ops": [] }
            })),
        );
        assert!(graph_revision_conflict.response.conflict);
        assert_response_diagnostic_code(
            &graph_revision_conflict.response,
            "graph.command.graph-revision-conflict",
        );

        let unsupported_view_target = apply_graph_command(
            &record,
            &identity,
            &frame,
            &graph_payload(json!({
                "kind": "view.patch",
                "target": package_patch_target("1"),
                "viewPatch": { "baseViewRevision": 0, "ops": [] }
            })),
        );
        assert_response_diagnostic_code(
            &unsupported_view_target.response,
            "graph.command.view-target-unsupported",
        );

        let target_revision_conflict = apply_graph_command(
            &record,
            &identity,
            &frame,
            &graph_payload(json!({
                "kind": "view.patch",
                "target": root_target("1"),
                "viewPatch": { "baseViewRevision": 0, "ops": [] }
            })),
        );
        assert!(target_revision_conflict.response.conflict);
        assert_response_diagnostic_code(
            &target_revision_conflict.response,
            "graph.command.target-revision-conflict",
        );

        let missing_change_target = apply_graph_command(
            &record,
            &identity,
            &frame,
            &graph_payload(json!({ "kind": "graph.changeSet" })),
        );
        assert_response_diagnostic_code(
            &missing_change_target.response,
            "graph.command.target-required",
        );

        let empty_changes = apply_graph_command(
            &record,
            &identity,
            &frame,
            &graph_payload(json!({
                "kind": "graph.changeSet",
                "target": root_target("1"),
                "changes": []
            })),
        );
        assert_response_diagnostic_code(&empty_changes.response, "graph.command.changes-required");

        let change_revision_conflict = apply_graph_command(
            &record,
            &identity,
            &frame,
            &graph_payload(json!({
                "kind": "graph.changeSet",
                "target": root_target("1"),
                "baseGraphRevision": "2",
                "changes": [
                    { "op": "node.delete", "changeId": "delete-value", "nodeId": "value_1" }
                ]
            })),
        );
        assert!(change_revision_conflict.response.conflict);
        assert_response_diagnostic_code(
            &change_revision_conflict.response,
            "graph.command.target-revision-conflict",
        );
    }

    #[test]
    fn cached_ack_and_control_event_helpers_preserve_payload_flags() {
        let registry = crate::RuntimeSessionRegistry::dry_preview();
        let record = registry.default_record();
        let identity = test_identity();
        let mut frame = client_frame(&record.id, "graph.command", "cached-ack", Value::Null);
        frame.idempotency_key = Some("idem-cached".to_owned());

        let cached = RuntimeRealtimeCachedCommandResult {
            event_cursor: "cursor-cached".to_owned(),
            ack_payload: json!({ "accepted": true, "cached": false }),
            emitted_results: Vec::new(),
        };
        let graph_ack = graph_ack_from_cached(&record, &identity, &frame, cached.clone());
        assert_eq!(graph_ack.message_type, "graph.ack");
        assert_eq!(graph_ack.payload["cached"], true);
        assert_eq!(graph_ack.payload["eventCursor"], "cursor-cached");

        let command_ack = command_ack_from_cached(&record, &identity, &frame, cached);
        assert_eq!(command_ack.message_type, "command.ack");
        assert_eq!(command_ack.payload["cached"], true);

        let request = RuntimeControlEventRequest {
            node_id: "value_1".to_owned(),
            port_id: "value".to_owned(),
            message: ControlMessage::bang(),
        };
        let mut response = RuntimeControlEventResponse {
            ok: true,
            changed: false,
            control_revision: Some(1),
            emitted: Vec::new(),
            diagnostics: Vec::new(),
        };
        assert!(
            control_emitted_event(
                &record,
                &identity,
                &frame,
                &request,
                &mut response,
                BTreeMap::new(),
                RealtimeEventPosition {
                    sequence: 3,
                    cursor: "cursor-3",
                },
            )
            .is_none()
        );

        let mut changed_values = BTreeMap::new();
        changed_values.insert("value_1".to_owned(), ControlValue::float(0.5));
        let event = control_emitted_event(
            &record,
            &identity,
            &frame,
            &request,
            &mut response,
            changed_values,
            RealtimeEventPosition {
                sequence: 4,
                cursor: "cursor-4",
            },
        )
        .expect("changed control values should produce an event");
        assert_eq!(event.message_type, "control.emitted");
        assert_eq!(event.sequence, Some(4));
        assert_eq!(event.payload["values"]["value_1"]["value"], 0.5);
    }
}
