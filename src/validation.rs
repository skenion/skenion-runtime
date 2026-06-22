use std::{error::Error, fmt};

use crate::{
    ApplyPatchError, DataType, GraphDocument, GraphPatch, GraphPatchOperation, InvertPatchError,
    NodeDefinition,
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
    schema_version_check("node definition", &definition.schema_version)
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

pub fn type_label(data_type: &DataType) -> String {
    skenion_contracts::type_label_v01(data_type)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn wraps_contract_validation_and_type_helpers() {
        let definition: NodeDefinition = serde_json::from_value(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "core.wrapper",
          "version": "0.1.0",
          "displayName": "Wrapper",
          "category": "Core",
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "value", "dataKind": "boolean" } }
          ],
          "execution": { "model": "value" },
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
              "kind": "core.wrapper",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "value", "dataKind": "boolean" } }
              ]
            }
          ],
          "edges": []
        }))
        .unwrap();
        let boolean_value = DataType {
            flow: crate::DataFlow::Value,
            data_kind: "boolean".to_owned(),
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
        assert_eq!(type_label(&boolean_value), "value<boolean>");
    }

    #[test]
    fn wraps_contract_validation_errors() {
        let invalid: NodeDefinition = serde_json::from_value(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "9.9.9",
          "id": "core.invalid",
          "version": "0.1.0",
          "displayName": "Invalid",
          "category": "Core",
          "ports": [],
          "execution": { "model": "value" },
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
              "kind": "core.wrapper",
              "kindVersion": "0.1.0",
              "params": { "value": 0.5 },
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "value", "dataKind": "boolean" } }
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
              "kind": "core.wrapper",
              "kindVersion": "0.1.0",
              "params": { "value": 0.5 },
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "value", "dataKind": "boolean" } }
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
}
