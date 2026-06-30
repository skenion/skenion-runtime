use serde_json::{Map, Value, json};

use super::super::wire::{RuntimeRealtimeConnectionIdentity, RuntimeRealtimeEnvelope};
use super::{
    GraphCommandOutcome, GraphCommandPayload, ObjectUnresolvedPolicy,
    graph_command_rejected_response,
};
use crate::object_spec::{
    ObjectRegistry, ObjectSpecPortActivation, ObjectSpecPortDirection, ObjectSpecPortRate,
    ObjectSpecResolution, materialize_object_spec_node_v01,
    materialize_unresolved_object_spec_node_v01, object_spec_node_definition_v01,
    unresolved_object_spec_node_definition_v01,
};
use crate::session::{ApplyObjectNodeCreateCurrentRequest, ApplyObjectNodeReplaceCurrentRequest};
use crate::{
    GraphTargetRef, PatchPath, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeDiagnostic, RuntimeMutationRequest, RuntimePatchResponse,
};

fn resolve_object_command_text(
    session: &crate::RuntimeSession,
    object_spec: &str,
) -> ObjectSpecResolution {
    let project = session.project_document_current();
    ObjectRegistry::for_project(project.as_ref()).resolve(object_spec)
}

pub(in crate::realtime) fn apply_object_resolve_graph_command(
    session: &crate::RuntimeSession,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let Some(object_spec) = payload.object_spec.as_deref() else {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.object-spec-required",
                "graph.command kind node.resolve requires payload.objectSpec",
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    let resolution = resolve_object_command_text(session, object_spec);
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
            diagnostics: object_spec_runtime_diagnostics(&resolution),
        },
        node_result,
    )
}

pub(in crate::realtime) fn apply_object_create_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let Some(object_spec) = payload.object_spec.as_deref() else {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.object-spec-required",
                "graph.command kind node.create requires payload.objectSpec",
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    let resolution = resolve_object_command_text(session, object_spec);
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
                    "object spec could not be resolved for node.create",
                    json!({
                        "commandKind": payload.kind,
                        "target": target,
                        "objectSpec": object_spec,
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

pub(in crate::realtime) fn apply_object_replace_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let Some(object_spec) = payload.object_spec.as_deref() else {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.object-spec-required",
                "graph.command kind node.replace requires payload.objectSpec",
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    let resolution = resolve_object_command_text(session, object_spec);
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
                    "object spec could not be resolved for node.replace",
                    json!({
                        "commandKind": payload.kind,
                        "target": target,
                        "nodeId": node_id,
                        "objectSpec": object_spec,
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

pub(in crate::realtime) fn apply_node_delete_graph_command(
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

pub(in crate::realtime) fn apply_node_update_graph_command(
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

pub(in crate::realtime) fn validate_object_command_target(
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
    resolution: &ObjectSpecResolution,
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

pub(in crate::realtime) fn node_id_slug(input: &str) -> Option<String> {
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

pub(in crate::realtime) fn next_generated_node_id(base: &str, used: &[String]) -> String {
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

pub(in crate::realtime) fn materialize_object_command_node(
    _session: &crate::RuntimeSession,
    payload: &GraphCommandPayload,
    resolution: &ObjectSpecResolution,
    node_id: &str,
) -> Option<(crate::GraphNodeCurrent, crate::NodeDefinitionCurrent)> {
    if resolution.ok() {
        let mut node = materialize_object_spec_node_v01(resolution, node_id).ok()?;
        merge_payload_params(&mut node.params, payload.params.as_ref());
        let definition = object_spec_node_definition_v01(resolution)?;
        return Some((node, definition));
    }
    if object_unresolved_policy(payload) == ObjectUnresolvedPolicy::MaterializeDiagnostic {
        let mut node = materialize_unresolved_object_spec_node_v01(resolution, node_id);
        merge_payload_params(&mut node.params, payload.params.as_ref());
        return Some((node, unresolved_object_spec_node_definition_v01()));
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

pub(in crate::realtime) fn object_spec_runtime_diagnostics(
    resolution: &ObjectSpecResolution,
) -> Vec<RuntimeDiagnostic> {
    resolution
        .diagnostics
        .iter()
        .map(|diagnostic| {
            RuntimeDiagnostic::structured_error(
                diagnostic.code.clone(),
                diagnostic.message.clone(),
                json!({
                    "surface": "object-spec",
                    "objectSpec": resolution.input,
                    "displayText": resolution.display_text,
                    "classSymbol": resolution.class_symbol,
                    "candidateCount": resolution.candidates.len(),
                    "candidates": resolution.candidates.iter().map(object_spec_candidate_json).collect::<Vec<_>>(),
                }),
            )
        })
        .collect()
}

pub(in crate::realtime) fn node_command_result(
    payload: &GraphCommandPayload,
    resolution: Option<&ObjectSpecResolution>,
    node_id: Option<&str>,
    dropped_edge_ids: Vec<String>,
    input: Option<Value>,
) -> Value {
    json!({
        "kind": payload.kind,
        "nodeId": node_id,
        "requestedNodeId": payload.requested_node_id,
        "target": payload.target,
        "objectSpec": payload.object_spec,
        "unresolvedPolicy": object_unresolved_policy(payload),
        "interfaceIncidentEdgePolicy": payload.interface_incident_edge_policy,
        "droppedEdgeIds": dropped_edge_ids,
        "resolution": resolution.map(object_resolution_json),
        "input": input,
    })
}

pub(in crate::realtime) fn node_input_result(
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

fn object_resolution_json(resolution: &ObjectSpecResolution) -> Value {
    json!({
        "input": resolution.input,
        "displayText": resolution.display_text,
        "classSymbol": resolution.class_symbol,
        "resolved": resolution.ok(),
        "resolvedKind": resolution.resolved_kind,
        "resolvedKindVersion": resolution.resolved_kind_version,
        "candidateCount": resolution.candidates.len(),
        "candidates": resolution.candidates.iter().map(object_spec_candidate_json).collect::<Vec<_>>(),
        "params": resolution.params,
        "ports": resolution.instance_ports.iter().map(object_spec_port_json).collect::<Vec<_>>(),
        "diagnostics": resolution.diagnostics.iter().map(|diagnostic| {
            json!({
                "code": diagnostic.code,
                "message": diagnostic.message,
            })
        }).collect::<Vec<_>>(),
    })
}

fn object_spec_candidate_json(candidate: &crate::object_spec::ObjectSpecCandidateSummary) -> Value {
    json!({
        "id": candidate.id,
        "source": candidate.source,
        "kind": candidate.kind,
        "displayName": candidate.display_name,
    })
}

fn object_spec_port_json(port: &crate::object_spec::ObjectSpecPort) -> Value {
    json!({
        "id": port.id,
        "direction": match &port.direction {
            ObjectSpecPortDirection::Input => "input",
            ObjectSpecPortDirection::Output => "output",
        },
        "type": port.port_type,
        "rate": match &port.rate {
            ObjectSpecPortRate::Event => "event",
            ObjectSpecPortRate::Control => "control",
            ObjectSpecPortRate::Audio => "audio",
            ObjectSpecPortRate::Render => "render",
            ObjectSpecPortRate::Gpu => "gpu",
            ObjectSpecPortRate::Resource => "resource",
            ObjectSpecPortRate::Io => "io",
        },
        "activation": port.activation.as_ref().map(|activation| match activation {
            ObjectSpecPortActivation::Trigger => "trigger",
            ObjectSpecPortActivation::Latched => "latched",
            ObjectSpecPortActivation::Passive => "passive",
        }),
    })
}
