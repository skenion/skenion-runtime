use serde_json::json;

use super::node_catalog::{node_catalog_changed_event, node_catalog_snapshot_for_session};
use super::protocol::*;
use super::state::{RememberAckInput, sync_required_issue};
use super::wire::{
    RuntimeRealtimeConnectionIdentity, RuntimeRealtimeEnvelope, RuntimeRealtimeIssue,
};
mod control;
mod events;
mod object_nodes;
mod outcome;
mod types;

use crate::runtime_time::created_at_now;
use crate::{
    IssueSeverity, PatchPath, RuntimeControlEventRequest, RuntimeIssue, RuntimeMutationRequest,
    RuntimeOperationAttribution, RuntimeOperationEnvelope, RuntimeOperationIssue,
    RuntimePatchResponse, RuntimeSessionRecord,
};
use control::{GraphControlEmission, apply_control_command};
pub(super) use events::{control_emitted_event, graph_ack_from_cached};
use events::{graph_ack, graph_applied_event};
pub(super) use outcome::GraphCommandOutcome;
pub(super) use types::{
    GraphCommandPayload, GraphEventContext, HistoryCommandScope, ObjectUnresolvedPolicy,
    RealtimeEventPosition,
};

pub(super) fn handle_graph_command(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<
    (
        RuntimeRealtimeEnvelope,
        Vec<RuntimeRealtimeEnvelope>,
        Vec<RuntimeRealtimeEnvelope>,
    ),
    RuntimeRealtimeIssue,
> {
    let idempotency_key = frame.idempotency_key.clone().ok_or_else(|| {
        sync_required_issue(
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
            sync_required_issue(
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

pub(super) fn apply_graph_command(
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
            RuntimeIssue::structured_error(
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
            RuntimeIssue::structured_error(
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
                    RuntimeIssue::structured_error(
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
                    RuntimeIssue::structured_error(
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
                    RuntimeIssue::structured_error(
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
            issues: response.issues.clone(),
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
                    RuntimeIssue::structured_error(
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
                    RuntimeIssue::structured_error(
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
                        RuntimeIssue::structured_error(
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
                        RuntimeIssue::structured_error(
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
                        RuntimeIssue::structured_error(
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
                    RuntimeIssue::structured_error(
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
                    RuntimeIssue::structured_error(
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
                    RuntimeIssue::structured_error(
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
            RuntimeIssue::structured_error(
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
            RuntimeIssue::structured_error(
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
        issues: operation_issues_to_runtime(&operation_result.issues),
    };
    GraphCommandOutcome::with_operation_result(response, operation_result)
}

fn operation_issues_to_runtime(issues: &[RuntimeOperationIssue]) -> Vec<RuntimeIssue> {
    issues
        .iter()
        .map(|issue| RuntimeIssue {
            severity: match issue.severity.as_str() {
                "warning" => IssueSeverity::Warning,
                "info" => IssueSeverity::Info,
                _ => IssueSeverity::Error,
            },
            message: issue.message.clone(),
            code: Some(issue.code.clone()),
            details: Some(json!({
                "path": issue.path.clone(),
                "target": issue.target.clone(),
                "expectedRevision": issue.expected_revision.clone(),
                "actualRevision": issue.actual_revision.clone(),
                "duplicates": issue.duplicates.clone(),
                "nodes": issue.nodes.clone(),
                "edges": issue.edges.clone(),
                "interfacePolicy": issue.interface_policy.clone(),
                "interfaceDetail": issue.interface_detail.clone(),
            })),
        })
        .collect()
}

pub(super) use object_nodes::{
    apply_node_delete_graph_command, apply_node_update_graph_command,
    apply_object_create_graph_command, apply_object_replace_graph_command,
    apply_object_resolve_graph_command, node_command_result, node_input_result,
};
#[cfg(test)]
pub(super) use object_nodes::{
    materialize_object_command_node, next_generated_node_id, node_id_slug,
    object_spec_runtime_issues, validate_object_command_target,
};

fn graph_command_rejected_response(
    session: &crate::RuntimeSession,
    conflict: bool,
    issue: RuntimeIssue,
) -> RuntimePatchResponse {
    RuntimePatchResponse {
        ok: false,
        applied: false,
        conflict,
        snapshot: session.snapshot(),
        history: session.history(),
        issues: vec![issue],
    }
}
