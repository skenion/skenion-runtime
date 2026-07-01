use serde_json::json;

use crate::{
    EdgeSpecCurrent, GraphDocumentCurrent, GraphTargetRef, PortDirectionCurrent, RuntimeIssue,
    port_type_accepts,
    session::{RuntimePatchResponse, RuntimeSession},
};

pub(super) fn node_target_revision_conflict_response(
    session: &RuntimeSession,
    target: &GraphTargetRef,
    actual_revision: &str,
) -> RuntimePatchResponse {
    session.patch_response(
        false,
        false,
        true,
        vec![RuntimeIssue::structured_error(
            "node.command.target-revision-conflict",
            format!(
                "target baseRevision {} does not match target graph revision {}",
                target.base_revision, actual_revision
            ),
            json!({
                "expectedRevision": target.base_revision,
                "actualRevision": actual_revision,
                "target": target,
            }),
        )],
    )
}

pub(super) fn invalid_incident_edge_ids_current(
    graph: &GraphDocumentCurrent,
    node_id: &str,
) -> Vec<String> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.source.node_id == node_id || edge.target.node_id == node_id)
        .filter(|edge| !edge_is_valid_current(graph, edge))
        .map(|edge| edge.id.clone())
        .collect()
}

fn edge_is_valid_current(graph: &GraphDocumentCurrent, edge: &EdgeSpecCurrent) -> bool {
    let Some(source_node) = graph
        .nodes
        .iter()
        .find(|node| node.id == edge.source.node_id)
    else {
        return false;
    };
    let Some(target_node) = graph
        .nodes
        .iter()
        .find(|node| node.id == edge.target.node_id)
    else {
        return false;
    };
    let Some(source_port) = source_node
        .ports
        .iter()
        .find(|port| port.id == edge.source.port_id)
    else {
        return false;
    };
    let Some(target_port) = target_node
        .ports
        .iter()
        .find(|port| port.id == edge.target.port_id)
    else {
        return false;
    };

    source_port.direction == PortDirectionCurrent::Output
        && target_port.direction == PortDirectionCurrent::Input
        && port_type_accepts(source_port, target_port)
}
