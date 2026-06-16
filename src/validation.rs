pub use skenion_contracts::{
    ValidationErrorV01 as ValidationError, ValidationReportV01 as ValidationReport,
};

use crate::{ApplyPatchError, DataType, GraphDocument, GraphPatch, NodeDefinition};

pub fn validate_node_definition(definition: &NodeDefinition) -> Result<(), ValidationReport> {
    skenion_contracts::validate_node_definition_v01(definition)
}

pub fn validate_graph_document(graph: &GraphDocument) -> Result<(), ValidationReport> {
    skenion_contracts::validate_graph_document_v01(graph)
}

pub fn apply_graph_patch(
    graph: &GraphDocument,
    patch: &GraphPatch,
    next_graph_revision: Option<&str>,
) -> Result<GraphDocument, ApplyPatchError> {
    skenion_contracts::apply_graph_patch_v01(graph, patch, next_graph_revision)
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
}
