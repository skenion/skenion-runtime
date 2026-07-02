use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};

use super::RealtimeDispatch;
use super::control_input::apply_control_input;
use super::protocol::{
    EVENT_CONTROL_EMITTED, FRAME_NODE_INPUT, RUNTIME_REALTIME_SCHEMA,
    RUNTIME_REALTIME_SCHEMA_VERSION,
};
use super::state::{RememberAckInput, sync_required_issue, validate_command_metadata};
use super::wire::{
    RuntimeRealtimeConnectionIdentity, RuntimeRealtimeEnvelope, RuntimeRealtimeIssue,
};
use super::{command_ack_with_payload, mark_ack_payload_cached};
use crate::runtime_time::created_at_now;
use crate::{
    ControlValue, RuntimeControlEmission, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeIssue, RuntimeSessionRecord,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NodeInputPayload {
    inputs: Vec<NodeInputRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NodeInputRequest {
    node_id: String,
    port_id: String,
    message: crate::ControlMessage,
}

struct AppliedNodeInput {
    request: RuntimeControlEventRequest,
    response: RuntimeControlEventResponse,
    changed_values: BTreeMap<String, ControlValue>,
}

pub(super) fn handle_node_input(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<RealtimeDispatch, RuntimeRealtimeIssue> {
    let idempotency_key = validate_command_metadata(&frame, "node.input")?;
    if let Some(cached) =
        record
            .realtime
            .cached_command_result(identity, &frame.message_type, idempotency_key)
    {
        let mut payload = mark_ack_payload_cached(cached.ack_payload);
        if let Some(object) = payload.as_object_mut() {
            object.insert("eventCursor".to_owned(), Value::String(cached.event_cursor));
        }
        return Ok(RealtimeDispatch::command(
            command_ack_with_payload(record, identity, &frame, payload),
            cached.emitted_results,
            Vec::new(),
        ));
    }

    let payload =
        serde_json::from_value::<NodeInputPayload>(frame.payload.clone()).map_err(|error| {
            sync_required_issue(
                "realtime.node-input.invalid-payload",
                format!("invalid node.input payload: {error}"),
                None,
            )
        })?;
    if payload.inputs.is_empty() {
        return Err(sync_required_issue(
            "realtime.node-input.inputs-required",
            "node.input payload inputs must not be empty",
            None,
        ));
    }

    let sequence = record.realtime.next_event_sequence();
    let cursor = record.realtime.cursor_for(sequence);
    let applied = payload
        .inputs
        .into_iter()
        .map(|input| {
            let request = RuntimeControlEventRequest {
                node_id: input.node_id,
                port_id: input.port_id,
                message: input.message,
            };
            let (response, changed_values, request) = apply_control_input(record, request);
            AppliedNodeInput {
                request,
                response,
                changed_values,
            }
        })
        .collect::<Vec<_>>();

    let accepted = applied.iter().all(|input| input.response.ok);
    let issues = realtime_issue_payloads(
        applied
            .iter()
            .flat_map(|input| input.response.issues.iter()),
    );
    let node_results = applied
        .iter()
        .enumerate()
        .map(|(index, input)| node_input_result(index, input))
        .collect::<Vec<_>>();
    let ack = command_ack_with_payload(
        record,
        identity,
        &frame,
        json!({
            "status": if accepted { "accepted" } else { "rejected" },
            "accepted": accepted,
            "applied": false,
            "conflict": false,
            "cached": false,
            "kind": FRAME_NODE_INPUT,
            "commandId": frame.command_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "correlationId": frame.correlation_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "idempotencyKey": frame.idempotency_key,
            "eventCursor": cursor,
            "node": { "inputs": node_results },
            "issues": issues,
        }),
    );
    let event =
        node_input_control_emitted_event(record, identity, &frame, &applied, sequence, &cursor);
    let emitted_results = event.iter().cloned().collect::<Vec<_>>();
    record.realtime.remember_ack(RememberAckInput {
        identity,
        message_type: &frame.message_type,
        idempotency_key,
        event_cursor: &cursor,
        event_sequence: sequence,
        ack_payload: ack.payload.clone(),
        emitted_results,
    });

    Ok(RealtimeDispatch::command(
        ack,
        Vec::new(),
        event.into_iter().collect(),
    ))
}

fn node_input_result(index: usize, input: &AppliedNodeInput) -> Value {
    json!({
        "index": index,
        "nodeId": input.request.node_id,
        "portId": input.request.port_id,
        "message": input.request.message,
        "accepted": input.response.ok,
        "changed": input.response.changed,
        "controlRevision": input.response.control_revision,
        "events": input.response.emitted,
        "issues": realtime_issue_payloads(input.response.issues.iter()),
    })
}

fn node_input_control_emitted_event(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    applied: &[AppliedNodeInput],
    sequence: u64,
    cursor: &str,
) -> Option<RuntimeRealtimeEnvelope> {
    let events = applied
        .iter()
        .flat_map(|input| input.response.emitted.iter().cloned())
        .collect::<Vec<RuntimeControlEmission>>();
    let values = applied
        .iter()
        .flat_map(|input| input.changed_values.iter())
        .map(|(node_id, value)| (node_id.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();

    if events.is_empty() && values.is_empty() {
        return None;
    }

    let issues = applied
        .iter()
        .flat_map(|input| input.response.issues.iter().cloned())
        .collect::<Vec<_>>();
    Some(runtime_control_emitted_event(
        record,
        Some(identity),
        Some(frame),
        RuntimeControlEmittedEventInput {
            sequence,
            cursor,
            control_revision: applied
                .iter()
                .rev()
                .find_map(|input| input.response.control_revision),
            changed: applied.iter().any(|input| input.response.changed),
            events,
            values,
            issues,
        },
    ))
}

pub(in crate::realtime) struct RuntimeControlEmittedEventInput<'a> {
    pub(in crate::realtime) sequence: u64,
    pub(in crate::realtime) cursor: &'a str,
    pub(in crate::realtime) control_revision: Option<u64>,
    pub(in crate::realtime) changed: bool,
    pub(in crate::realtime) events: Vec<RuntimeControlEmission>,
    pub(in crate::realtime) values: BTreeMap<String, ControlValue>,
    pub(in crate::realtime) issues: Vec<RuntimeIssue>,
}

pub(in crate::realtime) fn runtime_control_emitted_event(
    record: &RuntimeSessionRecord,
    identity: Option<&RuntimeRealtimeConnectionIdentity>,
    frame: Option<&RuntimeRealtimeEnvelope>,
    input: RuntimeControlEmittedEventInput<'_>,
) -> RuntimeRealtimeEnvelope {
    let mut payload = json!({
        "controlSequence": input.sequence,
        "controlRevision": input.control_revision,
        "changed": input.changed,
        "events": input.events,
        "values": if input.values.is_empty() { Value::Null } else { json!(input.values) },
        "issues": realtime_issue_payloads(input.issues.iter()),
        "replayed": false,
    });
    if let Some(frame) = frame
        && let Some(object) = payload.as_object_mut()
    {
        object.insert(
            "commandId".to_owned(),
            Value::String(
                frame
                    .command_id
                    .clone()
                    .unwrap_or_else(|| frame.message_id.clone()),
            ),
        );
        object.insert(
            "correlationId".to_owned(),
            Value::String(
                frame
                    .correlation_id
                    .clone()
                    .unwrap_or_else(|| frame.message_id.clone()),
            ),
        );
        if let Some(idempotency_key) = frame.idempotency_key.clone() {
            object.insert("idempotencyKey".to_owned(), Value::String(idempotency_key));
        }
    }

    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: EVENT_CONTROL_EMITTED.to_owned(),
        message_id: format!("{}_control_{:06}", record.id, input.sequence),
        session_id: record.id.clone(),
        connection_id: identity.map(|identity| identity.connection_id.clone()),
        client_id: identity.map(|identity| identity.client_id.clone()),
        window_id: identity.map(|identity| identity.window_id.clone()),
        command_id: frame.and_then(|frame| {
            frame
                .command_id
                .clone()
                .or_else(|| Some(frame.message_id.clone()))
        }),
        correlation_id: frame.and_then(|frame| {
            frame
                .correlation_id
                .clone()
                .or_else(|| Some(frame.message_id.clone()))
        }),
        idempotency_key: frame.and_then(|frame| frame.idempotency_key.clone()),
        sequence: Some(input.sequence),
        cursor: Some(input.cursor.to_owned()),
        created_at: Some(created_at_now()),
        payload,
    }
}

fn realtime_issue_payloads<'a>(issues: impl IntoIterator<Item = &'a RuntimeIssue>) -> Vec<Value> {
    issues
        .into_iter()
        .map(|issue| {
            let mut value = json!({
                "severity": issue.severity,
                "code": issue
                    .code
                    .as_deref()
                    .unwrap_or("runtime.control.issue"),
                "message": issue.message,
            });
            if let Some(details) = issue.details.clone()
                && let Some(object) = value.as_object_mut()
            {
                object.insert("details".to_owned(), details);
            }
            value
        })
        .collect()
}
