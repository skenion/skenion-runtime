use std::{error::Error, fmt};

use crate::{
    ApplyPatchError, DataType, GraphDocument, GraphPatch, GraphPatchOperation, InvertPatchError,
    NodeDefinition, PortSpecCurrent,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    errors: Vec<ValidationError>,
}

impl ValidationReport {
    fn new(message: impl Into<String>) -> Self {
        Self {
            errors: vec![ValidationError {
                message: message.into(),
            }],
        }
    }

    fn from_errors(errors: Vec<ValidationError>) -> Self {
        Self { errors }
    }

    pub fn errors(&self) -> &[ValidationError] {
        &self.errors
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let messages = self
            .errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        write!(f, "{messages}")
    }
}

impl Error for ValidationReport {}

pub fn validate_node_definition(definition: &NodeDefinition) -> Result<(), ValidationReport> {
    let mut errors = Vec::new();
    if let Err(report) = schema_version_check("node definition", &definition.schema_version) {
        errors.extend(report.errors);
    }
    if is_payload_identity_node_kind(definition.id.as_str()) {
        errors.push(ValidationError {
            message: format!("payload identity node definition id: {}", definition.id),
        });
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ValidationReport::from_errors(errors))
    }
}

pub fn validate_graph_document(graph: &GraphDocument) -> Result<(), ValidationReport> {
    let mut errors = Vec::new();
    if let Err(report) = schema_version_check("graph", &graph.schema_version) {
        errors.extend(report.errors);
    }

    let mut seen = std::collections::HashSet::new();
    for node in &graph.nodes {
        if !seen.insert(node.id.as_str()) {
            errors.push(ValidationError {
                message: format!("duplicate node id: {}", node.id),
            });
        }
        if is_payload_identity_node_kind(node.kind.as_str()) {
            errors.push(ValidationError {
                message: format!(
                    "node {} uses payload identity {} as an executable kind",
                    node.id, node.kind
                ),
            });
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ValidationReport::from_errors(errors))
    }
}

fn schema_version_check(surface: &str, schema_version: &str) -> Result<(), ValidationReport> {
    if schema_version == "0.1.0" {
        Ok(())
    } else {
        Err(ValidationReport::new(format!(
            "{surface} schemaVersion must be 0.1.0, received {schema_version}"
        )))
    }
}

fn is_payload_identity_node_kind(kind: &str) -> bool {
    matches!(
        kind,
        "value"
            | "data"
            | "payload"
            | "bool"
            | "string"
            | "object.core.bool"
            | "object.core.string"
            | "value.core.message"
            | "value.core.bang"
            | "value.core.string"
            | "value.core.tensor"
    ) || kind.starts_with("value.")
        || kind.starts_with("data.")
        || kind.starts_with("payload.")
        || kind.starts_with("control.")
}

pub fn apply_graph_patch(
    graph: &GraphDocument,
    patch: &GraphPatch,
    next_graph_revision: Option<&str>,
) -> Result<GraphDocument, ApplyPatchError> {
    if patch.base_revision != graph.revision {
        return Err(ApplyPatchError::new(format!(
            "base revision mismatch: expected {}, received {}",
            graph.revision, patch.base_revision
        )));
    }

    let mut graph = graph.clone();
    for op in &patch.ops {
        match op {
            GraphPatchOperation::SetNodeParam {
                node_id,
                key,
                value,
            } => {
                let Some(node) = graph.nodes.iter_mut().find(|node| node.id == *node_id) else {
                    return Err(ApplyPatchError::new(format!("missing node: {node_id}")));
                };
                node.params.insert(key.clone(), value.clone());
            }
            GraphPatchOperation::AddNode { node } => {
                if graph.nodes.iter().any(|existing| existing.id == node.id) {
                    return Err(ApplyPatchError::new(format!("duplicate node: {}", node.id)));
                }
                graph.nodes.push(node.clone());
            }
            GraphPatchOperation::RemoveNode { node_id } => {
                let before = graph.nodes.len();
                graph.nodes.retain(|node| node.id != *node_id);
                if graph.nodes.len() == before {
                    return Err(ApplyPatchError::new(format!("missing node: {node_id}")));
                }
                graph
                    .edges
                    .retain(|edge| edge.from.node != *node_id && edge.to.node != *node_id);
            }
            GraphPatchOperation::ReplaceNode { node_id, node } => {
                let Some(existing) = graph.nodes.iter_mut().find(|node| node.id == *node_id) else {
                    return Err(ApplyPatchError::new(format!("missing node: {node_id}")));
                };
                *existing = node.clone();
            }
            GraphPatchOperation::AddEdge { edge } => {
                if graph.edges.iter().any(|existing| existing == edge) {
                    return Err(ApplyPatchError::new("duplicate edge"));
                }
                graph.edges.push(edge.clone());
            }
            GraphPatchOperation::RemoveEdge { edge } => {
                let before = graph.edges.len();
                graph.edges.retain(|existing| existing != edge);
                if graph.edges.len() == before {
                    return Err(ApplyPatchError::new("missing edge"));
                }
            }
        }
    }

    if let Some(revision) = next_graph_revision {
        graph.revision = revision.to_owned();
    }

    Ok(graph)
}

pub fn invert_graph_patch(
    graph_before: &GraphDocument,
    patch: &GraphPatch,
) -> Result<GraphPatch, InvertPatchError> {
    let mut inverse_ops = Vec::new();
    for op in patch.ops.iter().rev() {
        match op {
            GraphPatchOperation::SetNodeParam { node_id, key, .. } => {
                let Some(node) = graph_before.nodes.iter().find(|node| node.id == *node_id) else {
                    return Err(InvertPatchError::new(format!("missing node: {node_id}")));
                };
                let value = node
                    .params
                    .get(key)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                inverse_ops.push(GraphPatchOperation::SetNodeParam {
                    node_id: node_id.clone(),
                    key: key.clone(),
                    value,
                });
            }
            GraphPatchOperation::AddNode { node } => {
                inverse_ops.push(GraphPatchOperation::RemoveNode {
                    node_id: node.id.clone(),
                });
            }
            GraphPatchOperation::RemoveNode { node_id } => {
                let Some(node) = graph_before.nodes.iter().find(|node| node.id == *node_id) else {
                    return Err(InvertPatchError::new(format!("missing node: {node_id}")));
                };
                inverse_ops.push(GraphPatchOperation::AddNode { node: node.clone() });
            }
            GraphPatchOperation::ReplaceNode { node_id, .. } => {
                let Some(node) = graph_before.nodes.iter().find(|node| node.id == *node_id) else {
                    return Err(InvertPatchError::new(format!("missing node: {node_id}")));
                };
                inverse_ops.push(GraphPatchOperation::ReplaceNode {
                    node_id: node_id.clone(),
                    node: node.clone(),
                });
            }
            GraphPatchOperation::AddEdge { edge } => {
                inverse_ops.push(GraphPatchOperation::RemoveEdge { edge: edge.clone() });
            }
            GraphPatchOperation::RemoveEdge { edge } => {
                inverse_ops.push(GraphPatchOperation::AddEdge { edge: edge.clone() });
            }
        }
    }

    Ok(GraphPatch {
        schema: patch.schema.clone(),
        schema_version: patch.schema_version.clone(),
        id: format!("{}-inverse", patch.id),
        base_revision: (graph_before.revision.parse::<u64>().unwrap_or_default() + 1).to_string(),
        ops: inverse_ops,
    })
}

pub fn compatible_data_types(source_type: &DataType, target_type: &DataType) -> bool {
    skenion_contracts::compatible_data_types_v01(source_type, target_type)
}

pub fn port_type_accepts(source: &PortSpecCurrent, target: &PortSpecCurrent) -> bool {
    skenion_contracts::port_type_accepts_v01(source, target)
}

pub fn port_connection_policy(
    source: &PortSpecCurrent,
    target: &PortSpecCurrent,
) -> skenion_contracts::PortConnectionPolicyV01 {
    skenion_contracts::port_connection_policy_v01(source, target)
}

pub fn type_label(data_type: &DataType) -> String {
    skenion_contracts::type_label_v01(data_type)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{Edge, GraphNode, PortRef};

    use super::*;

    #[test]
    fn wraps_contract_validation_and_type_helpers() {
        let definition: NodeDefinition = serde_json::from_value(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.wrapper",
          "version": "0.1.0",
          "displayName": "Wrapper",
          "category": "Core",
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.bool" } }
          ],
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
        .unwrap();
        let graph: GraphDocument = serde_json::from_value(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "wrapper",
          "revision": "1",
          "nodes": [
            {
              "id": "node",
              "kind": "object.core.wrapper",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.bool" } }
              ]
            }
          ],
          "edges": []
        }))
        .unwrap();
        let boolean_value = DataType {
            flow: crate::DataFlow::Control,
            data_kind: "value.core.bool".to_owned(),
            unit: None,
            range: None,
            shape: None,
            channels: None,
            sample_rate: None,
            format: None,
            color_space: None,
            frame_rate: None,
            alpha_policy: None,
            values: None,
        };

        assert!(validate_node_definition(&definition).is_ok());
        assert!(validate_graph_document(&graph).is_ok());
        assert!(compatible_data_types(&boolean_value, &boolean_value));
        assert_eq!(type_label(&boolean_value), "control<value.core.bool>");
    }

    #[test]
    fn rejects_payload_identity_node_kinds_and_definition_ids() {
        for payload_identity in [
            "object.core.bool",
            "object.core.string",
            "bool",
            "string",
            "value.number",
            "value.core.message",
            "value.core.bang",
            "value.core.string",
            "value.core.tensor",
        ] {
            let definition: NodeDefinition = serde_json::from_value(json!({
                "schema": "skenion.node.definition",
                "schemaVersion": "0.1.0",
                "id": payload_identity,
                "version": "0.1.0",
                "displayName": "Payload Identity",
                "category": "Core",
                "ports": [],
                "execution": { "model": "control" },
                "state": { "persistent": false },
                "permissions": [],
                "capabilities": []
            }))
            .unwrap();
            let definition_report = validate_node_definition(&definition)
                .expect_err("payload identity definition id should fail");
            assert!(
                definition_report
                    .to_string()
                    .contains("payload identity node definition id"),
                "{payload_identity}: {definition_report}"
            );

            let mut graph = patch_graph();
            graph.nodes[0].kind = payload_identity.to_owned();
            let graph_report = validate_graph_document(&graph)
                .expect_err("payload identity graph node kind should fail");
            assert!(
                graph_report.to_string().contains("uses payload identity"),
                "{payload_identity}: {graph_report}"
            );
        }
    }

    #[test]
    fn wraps_contract_validation_errors() {
        let invalid: NodeDefinition = serde_json::from_value(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "9.9.9",
          "id": "object.core.invalid",
          "version": "0.1.0",
          "displayName": "Invalid",
          "category": "Core",
          "ports": [],
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
        .unwrap();

        let report = validate_node_definition(&invalid).unwrap_err();

        assert!(!report.errors().is_empty());
        assert!(!report.to_string().is_empty());
    }

    #[test]
    fn wraps_contract_graph_patch_application() {
        let graph: GraphDocument = serde_json::from_value(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "wrapper-patch",
          "revision": "1",
          "nodes": [
            {
              "id": "node",
              "kind": "object.core.wrapper",
              "kindVersion": "0.1.0",
              "params": { "value": 0.5 },
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.bool" } }
              ]
            }
          ],
          "edges": []
        }))
        .unwrap();
        let patch: GraphPatch = serde_json::from_value(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "patch",
          "baseRevision": "1",
          "ops": [
            { "op": "setNodeParam", "nodeId": "node", "key": "value", "value": true }
          ]
        }))
        .unwrap();

        let patched = apply_graph_patch(&graph, &patch, Some("2")).unwrap();

        assert_eq!(patched.revision, "2");
        assert_eq!(patched.nodes[0].params["value"], Value::Bool(true));
    }

    #[test]
    fn wraps_contract_graph_patch_inversion() {
        let graph: GraphDocument = serde_json::from_value(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "wrapper-invert",
          "revision": "1",
          "nodes": [
            {
              "id": "node",
              "kind": "object.core.wrapper",
              "kindVersion": "0.1.0",
              "params": { "value": 0.5 },
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.bool" } }
              ]
            }
          ],
          "edges": []
        }))
        .unwrap();
        let patch: GraphPatch = serde_json::from_value(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "patch",
          "baseRevision": "1",
          "ops": [
            { "op": "setNodeParam", "nodeId": "node", "key": "value", "value": true }
          ]
        }))
        .unwrap();

        let inverse = invert_graph_patch(&graph, &patch).unwrap();

        assert_eq!(inverse.base_revision, "2");
        assert_eq!(inverse.ops.len(), 1);
    }

    #[test]
    fn graph_validation_reports_schema_and_duplicate_node_errors() {
        let mut graph = patch_graph();
        graph.schema_version = "9.9.9".to_owned();
        graph.nodes.push(patch_node("node"));

        let report = validate_graph_document(&graph).unwrap_err();
        let messages = report
            .errors()
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>();

        assert!(
            messages
                .iter()
                .any(|message| message.contains("graph schemaVersion must be 0.1.0"))
        );
        assert!(messages.contains(&"duplicate node id: node"));
        assert!(report.to_string().contains("; "));
    }

    #[test]
    fn graph_patch_application_handles_node_and_edge_mutations() {
        let graph = patch_graph();
        let node_to_target = patch_edge("node", "target");
        let node_to_extra = patch_edge("node", "extra");
        let mut replacement = patch_node("target");
        replacement
            .params
            .insert("label".to_owned(), Value::String("replaced".to_owned()));
        let patch = patch_with_ops(
            "1",
            vec![
                GraphPatchOperation::AddNode {
                    node: patch_node("extra"),
                },
                GraphPatchOperation::AddEdge {
                    edge: node_to_target.clone(),
                },
                GraphPatchOperation::RemoveEdge {
                    edge: node_to_target,
                },
                GraphPatchOperation::AddEdge {
                    edge: node_to_extra,
                },
                GraphPatchOperation::RemoveNode {
                    node_id: "extra".to_owned(),
                },
                GraphPatchOperation::ReplaceNode {
                    node_id: "target".to_owned(),
                    node: replacement,
                },
                GraphPatchOperation::SetNodeParam {
                    node_id: "node".to_owned(),
                    key: "enabled".to_owned(),
                    value: Value::Bool(true),
                },
            ],
        );

        let patched = apply_graph_patch(&graph, &patch, None).unwrap();

        assert_eq!(patched.revision, "1");
        assert_eq!(patched.nodes.len(), 2);
        assert!(patched.edges.is_empty());
        assert_eq!(patched.nodes[0].params["enabled"], Value::Bool(true));
        assert_eq!(
            patched.nodes[1].params["label"],
            Value::String("replaced".to_owned())
        );
    }

    #[test]
    fn graph_patch_application_reports_structured_error_cases() {
        let graph = patch_graph();
        let existing_edge = patch_edge("node", "target");
        let mut graph_with_edge = graph.clone();
        graph_with_edge.edges.push(existing_edge.clone());

        assert_apply_error(
            &graph,
            patch_with_ops("0", Vec::new()),
            "base revision mismatch",
        );
        assert_apply_error(
            &graph,
            patch_with_ops(
                "1",
                vec![GraphPatchOperation::SetNodeParam {
                    node_id: "missing".to_owned(),
                    key: "value".to_owned(),
                    value: Value::Bool(true),
                }],
            ),
            "missing node: missing",
        );
        assert_apply_error(
            &graph,
            patch_with_ops(
                "1",
                vec![GraphPatchOperation::AddNode {
                    node: patch_node("node"),
                }],
            ),
            "duplicate node: node",
        );
        assert_apply_error(
            &graph,
            patch_with_ops(
                "1",
                vec![GraphPatchOperation::RemoveNode {
                    node_id: "missing".to_owned(),
                }],
            ),
            "missing node: missing",
        );
        assert_apply_error(
            &graph,
            patch_with_ops(
                "1",
                vec![GraphPatchOperation::ReplaceNode {
                    node_id: "missing".to_owned(),
                    node: patch_node("replacement"),
                }],
            ),
            "missing node: missing",
        );
        assert_apply_error(
            &graph_with_edge,
            patch_with_ops(
                "1",
                vec![GraphPatchOperation::AddEdge {
                    edge: existing_edge,
                }],
            ),
            "duplicate edge",
        );
        assert_apply_error(
            &graph,
            patch_with_ops(
                "1",
                vec![GraphPatchOperation::RemoveEdge {
                    edge: patch_edge("node", "target"),
                }],
            ),
            "missing edge",
        );
    }

    #[test]
    fn graph_patch_inversion_handles_all_operation_shapes_and_missing_nodes() {
        let graph = patch_graph();
        let added_edge = patch_edge("node", "target");
        let removed_edge = patch_edge("target", "node");
        let mut replacement = patch_node("node");
        replacement
            .params
            .insert("value".to_owned(), Value::String("new".to_owned()));
        let patch = patch_with_ops(
            "1",
            vec![
                GraphPatchOperation::SetNodeParam {
                    node_id: "node".to_owned(),
                    key: "missingKey".to_owned(),
                    value: Value::Bool(true),
                },
                GraphPatchOperation::AddNode {
                    node: patch_node("extra"),
                },
                GraphPatchOperation::RemoveNode {
                    node_id: "target".to_owned(),
                },
                GraphPatchOperation::ReplaceNode {
                    node_id: "node".to_owned(),
                    node: replacement,
                },
                GraphPatchOperation::AddEdge {
                    edge: added_edge.clone(),
                },
                GraphPatchOperation::RemoveEdge {
                    edge: removed_edge.clone(),
                },
            ],
        );

        let inverse = invert_graph_patch(&graph, &patch).unwrap();

        assert_eq!(inverse.id, "patch-inverse");
        assert_eq!(inverse.base_revision, "2");
        assert_eq!(inverse.ops.len(), 6);
        assert_eq!(
            inverse.ops[0],
            GraphPatchOperation::AddEdge { edge: removed_edge }
        );
        assert_eq!(
            inverse.ops[1],
            GraphPatchOperation::RemoveEdge { edge: added_edge }
        );
        assert_eq!(
            inverse.ops[4],
            GraphPatchOperation::RemoveNode {
                node_id: "extra".to_owned()
            }
        );
        assert_eq!(
            inverse.ops[5],
            GraphPatchOperation::SetNodeParam {
                node_id: "node".to_owned(),
                key: "missingKey".to_owned(),
                value: Value::Null
            }
        );

        assert_invert_error(
            &graph,
            GraphPatchOperation::SetNodeParam {
                node_id: "missing".to_owned(),
                key: "value".to_owned(),
                value: Value::Bool(true),
            },
            "missing node: missing",
        );
        assert_invert_error(
            &graph,
            GraphPatchOperation::RemoveNode {
                node_id: "missing".to_owned(),
            },
            "missing node: missing",
        );
        assert_invert_error(
            &graph,
            GraphPatchOperation::ReplaceNode {
                node_id: "missing".to_owned(),
                node: patch_node("replacement"),
            },
            "missing node: missing",
        );
    }

    fn assert_apply_error(graph: &GraphDocument, patch: GraphPatch, expected: &str) {
        let error = apply_graph_patch(graph, &patch, None).unwrap_err();
        assert!(
            error.message.contains(expected),
            "expected {expected:?}, received {:?}",
            error.message
        );
    }

    fn assert_invert_error(graph: &GraphDocument, op: GraphPatchOperation, expected_message: &str) {
        let patch = patch_with_ops("1", vec![op]);
        let error = invert_graph_patch(graph, &patch).unwrap_err();
        assert_eq!(error.message, expected_message);
    }

    fn patch_graph() -> GraphDocument {
        GraphDocument {
            schema: "skenion.graph".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "patch-graph".to_owned(),
            revision: "1".to_owned(),
            nodes: vec![patch_node("node"), patch_node("target")],
            edges: Vec::new(),
        }
    }

    fn patch_node(id: &str) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            kind: "object.core.wrapper".to_owned(),
            kind_version: "0.1.0".to_owned(),
            params: serde_json::Map::new(),
            ports: Vec::new(),
        }
    }

    fn patch_edge(from_node: &str, to_node: &str) -> Edge {
        Edge {
            from: PortRef {
                node: from_node.to_owned(),
                port: "out".to_owned(),
            },
            to: PortRef {
                node: to_node.to_owned(),
                port: "in".to_owned(),
            },
        }
    }

    fn patch_with_ops(base_revision: &str, ops: Vec<GraphPatchOperation>) -> GraphPatch {
        GraphPatch {
            schema: "skenion.graph.patch".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "patch".to_owned(),
            base_revision: base_revision.to_owned(),
            ops,
        }
    }
}
