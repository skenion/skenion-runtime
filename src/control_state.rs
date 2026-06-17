use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    ControlValue, GraphDocument, RuntimeDiagnostic,
    control_value::{COLOR_RGBA_KIND, VALUE_BOOL_KIND, VALUE_F32_KIND, VALUE_I32_KIND},
};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    pub values: BTreeMap<String, ControlValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlEventRequest {
    pub node_id: String,
    pub port_id: String,
    pub value: ControlValue,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlEmission {
    pub node_id: String,
    pub port_id: String,
    pub value: ControlValue,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlEventResponse {
    pub ok: bool,
    pub emitted: Vec<RuntimeControlEmission>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlStateResponse {
    pub ok: bool,
    pub values: BTreeMap<String, ControlValue>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

impl ControlState {
    pub fn from_graph(graph: &GraphDocument) -> Self {
        let values = graph
            .nodes
            .iter()
            .filter_map(|node| {
                ControlValue::for_node_default(node).map(|value| (node.id.clone(), value))
            })
            .collect();
        Self { values }
    }

    pub fn value_for_node(&self, node_id: &str) -> Option<&ControlValue> {
        self.values.get(node_id)
    }

    pub fn apply_event(
        &mut self,
        request: RuntimeControlEventRequest,
        graph: &GraphDocument,
    ) -> RuntimeControlEventResponse {
        let Some(node) = graph.nodes.iter().find(|node| node.id == request.node_id) else {
            return RuntimeControlEventResponse::error(format!(
                "control node {} does not exist",
                request.node_id
            ));
        };

        if !is_control_value_kind(&node.kind) {
            return RuntimeControlEventResponse::error(format!(
                "node {} ({}) does not support runtime control events",
                node.id, node.kind
            ));
        }

        let Some(stored) = self.values.get(&node.id).cloned() else {
            return RuntimeControlEventResponse::error(format!(
                "node {} has no runtime control state",
                node.id
            ));
        };

        match request.port_id.as_str() {
            "set" => {
                if let Err(diagnostic) = ensure_value_type(&request.value, &stored, &node.id) {
                    return RuntimeControlEventResponse::error(diagnostic);
                }
                self.values.insert(node.id.clone(), request.value);
                RuntimeControlEventResponse::ok(Vec::new())
            }
            "in" => {
                if let Err(diagnostic) = ensure_value_type(&request.value, &stored, &node.id) {
                    return RuntimeControlEventResponse::error(diagnostic);
                }
                self.values.insert(node.id.clone(), request.value.clone());
                RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                    node_id: node.id.clone(),
                    port_id: "value".to_owned(),
                    value: request.value,
                }])
            }
            "bang" => {
                if !matches!(request.value, ControlValue::Bang) {
                    return RuntimeControlEventResponse::error(format!(
                        "control input {}.bang expects bang, got {}",
                        node.id,
                        request.value.kind_label()
                    ));
                }
                RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                    node_id: node.id.clone(),
                    port_id: "value".to_owned(),
                    value: stored,
                }])
            }
            port => RuntimeControlEventResponse::error(format!(
                "node {} does not support runtime control input port {}",
                node.id, port
            )),
        }
    }
}

impl RuntimeControlEventResponse {
    fn ok(emitted: Vec<RuntimeControlEmission>) -> Self {
        Self {
            ok: true,
            emitted,
            diagnostics: Vec::new(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            emitted: Vec::new(),
            diagnostics: vec![RuntimeDiagnostic::error(message)],
        }
    }
}

pub fn is_control_value_kind(kind: &str) -> bool {
    matches!(
        kind,
        VALUE_F32_KIND | VALUE_I32_KIND | VALUE_BOOL_KIND | COLOR_RGBA_KIND
    )
}

fn ensure_value_type(
    value: &ControlValue,
    stored: &ControlValue,
    node_id: &str,
) -> Result<(), String> {
    if value.matches_stored_type(stored) {
        return Ok(());
    }

    Err(format!(
        "control input {node_id} expects {}, got {}",
        stored.kind_label(),
        value.kind_label()
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;
    use crate::GraphNode;

    #[test]
    fn initializes_control_values_from_graph() {
        let state = ControlState::from_graph(&graph(vec![
            value_node("f32", VALUE_F32_KIND, json!(1.25)),
            value_node("i32", VALUE_I32_KIND, json!(7)),
            value_node("bool", VALUE_BOOL_KIND, json!(true)),
            value_node("rgba", COLOR_RGBA_KIND, json!([0.1, 0.2, 0.3, 1.0])),
            value_node("other", "core.target", json!(10)),
        ]));

        assert_eq!(state.values.len(), 4);
        assert_eq!(state.value_for_node("f32"), Some(&ControlValue::F32(1.25)));
        assert_eq!(state.value_for_node("i32"), Some(&ControlValue::I32(7)));
        assert_eq!(
            state.value_for_node("rgba"),
            Some(&ControlValue::Rgba([0.1, 0.2, 0.3, 1.0]))
        );
        assert_eq!(state.value_for_node("other"), None);
    }

    #[test]
    fn set_updates_without_emission() {
        let graph = graph(vec![value_node("value_1", VALUE_F32_KIND, json!(1.0))]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "value_1".to_owned(),
                port_id: "set".to_owned(),
                value: ControlValue::F32(32.0),
            },
            &graph,
        );

        assert!(response.ok);
        assert!(response.emitted.is_empty());
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::F32(32.0))
        );
    }

    #[test]
    fn in_updates_and_emits() {
        let graph = graph(vec![value_node("value_1", VALUE_I32_KIND, json!(1))]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "value_1".to_owned(),
                port_id: "in".to_owned(),
                value: ControlValue::I32(12),
            },
            &graph,
        );

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![RuntimeControlEmission {
                node_id: "value_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::I32(12)
            }]
        );
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::I32(12))
        );
    }

    #[test]
    fn bang_emits_stored_value_without_update() {
        let graph = graph(vec![value_node("value_1", VALUE_BOOL_KIND, json!(true))]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "value_1".to_owned(),
                port_id: "bang".to_owned(),
                value: ControlValue::Bang,
            },
            &graph,
        );

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![RuntimeControlEmission {
                node_id: "value_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::Bool(true)
            }]
        );
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::Bool(true))
        );
    }

    #[test]
    fn invalid_events_do_not_mutate_state() {
        let graph = graph(vec![value_node("value_1", VALUE_F32_KIND, json!(1.0))]);
        let mut state = ControlState::from_graph(&graph);

        for request in [
            RuntimeControlEventRequest {
                node_id: "missing".to_owned(),
                port_id: "set".to_owned(),
                value: ControlValue::F32(2.0),
            },
            RuntimeControlEventRequest {
                node_id: "value_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::F32(2.0),
            },
            RuntimeControlEventRequest {
                node_id: "value_1".to_owned(),
                port_id: "set".to_owned(),
                value: ControlValue::Bool(true),
            },
            RuntimeControlEventRequest {
                node_id: "value_1".to_owned(),
                port_id: "bang".to_owned(),
                value: ControlValue::F32(2.0),
            },
        ] {
            let response = state.apply_event(request, &graph);
            assert!(!response.ok);
            assert!(response.emitted.is_empty());
            assert!(!response.diagnostics.is_empty());
            assert_eq!(
                state.value_for_node("value_1"),
                Some(&ControlValue::F32(1.0))
            );
        }
    }

    #[test]
    fn rejects_non_control_nodes_and_missing_control_state() {
        let graph = graph(vec![
            value_node("value_1", VALUE_F32_KIND, json!(1.0)),
            value_node("target_1", "core.target", json!(1.0)),
        ]);

        let mut state = ControlState::from_graph(&graph);
        let non_control = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "target_1".to_owned(),
                port_id: "set".to_owned(),
                value: ControlValue::F32(2.0),
            },
            &graph,
        );
        assert!(!non_control.ok);
        assert!(
            non_control.diagnostics[0]
                .message
                .contains("does not support runtime control events")
        );

        state.values.remove("value_1");
        let missing_state = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "value_1".to_owned(),
                port_id: "set".to_owned(),
                value: ControlValue::F32(2.0),
            },
            &graph,
        );
        assert!(!missing_state.ok);
        assert!(
            missing_state.diagnostics[0]
                .message
                .contains("has no runtime control state")
        );
    }

    fn graph(nodes: Vec<GraphNode>) -> GraphDocument {
        GraphDocument {
            schema: "skenion.graph".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "control-graph".to_owned(),
            revision: "1".to_owned(),
            nodes,
            edges: Vec::new(),
        }
    }

    fn value_node(id: &str, kind: &str, value: serde_json::Value) -> GraphNode {
        let mut params = Map::new();
        params.insert("value".to_owned(), value);
        GraphNode {
            id: id.to_owned(),
            kind: kind.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: Vec::new(),
        }
    }
}
