use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::super::protocol::*;
use super::super::state::RuntimeRealtimeCachedCommandResult;
use super::super::wire::{RuntimeRealtimeConnectionIdentity, RuntimeRealtimeEnvelope};
use super::super::{graph_ack_with_payload, mark_ack_payload_cached};
use super::{GraphEventContext, RealtimeEventPosition};
use crate::runtime_time::created_at_now;
use crate::{
    ControlValue, RuntimeControlEventRequest, RuntimeControlEventResponse, RuntimeSessionRecord,
};

pub(in crate::realtime) fn control_emitted_event(
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
            "issues": response.issues,
            "replayed": false,
        }),
    })
}

pub(super) fn graph_ack(context: &GraphEventContext<'_>, cached: bool) -> RuntimeRealtimeEnvelope {
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
            "issues": context.response.issues,
        }),
    )
}

pub(in crate::realtime) fn graph_ack_from_cached(
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

pub(super) fn graph_applied_event(context: &GraphEventContext<'_>) -> RuntimeRealtimeEnvelope {
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
            "issues": context.response.issues,
            "replayed": false,
        }),
    }
}
