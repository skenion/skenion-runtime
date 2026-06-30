use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::{Value, json};

use super::protocol::{
    EVENT_PRESENCE_UPDATED, EVENT_SELECTION_UPDATED, RUNTIME_REALTIME_SCHEMA,
    RUNTIME_REALTIME_SCHEMA_VERSION,
};
use super::state::{RememberAckInput, sync_required_diagnostic};
use super::wire::{
    RuntimeRealtimeConnectionIdentity, RuntimeRealtimeDiagnostic, RuntimeRealtimeEnvelope,
};
use super::{command_ack, command_ack_from_cached, unix_ms_timestamp};
use crate::runtime_time::created_at_now;
use crate::{
    GraphTargetRef, RuntimeCollaborationCursor, RuntimeCollaborationSelection,
    RuntimeCollaborationSelectionEnvelope, RuntimeSessionRecord,
    validate_runtime_collaboration_selection_envelope,
};

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

pub(super) fn handle_presence_update(
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

pub(super) fn handle_selection_update(
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
