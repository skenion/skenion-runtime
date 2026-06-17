use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ControlValue, GraphDocument, GraphNode, PortDirection, RuntimeDiagnostic,
    control_value::{
        COLOR_RGBA_KIND, MESSAGE_KIND, RECEIVE_BOOL_KIND, RECEIVE_F32_KIND, RECEIVE_I32_KIND,
        RECEIVE_RGBA_KIND, SEND_BOOL_KIND, SEND_F32_KIND, SEND_I32_KIND, SEND_RGBA_KIND,
        STRING_KIND, TOGGLE_KIND, UI_BUTTON_KIND, UI_SLIDER_F32_KIND, UI_TOGGLE_KIND,
        VALUE_BOOL_KIND, VALUE_F32_KIND, VALUE_I32_KIND,
    },
};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    pub values: BTreeMap<String, ControlValue>,
    pub channels: BTreeMap<String, ControlValue>,
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
    pub channels: BTreeMap<String, ControlValue>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeControlReadTarget {
    Param,
    Port,
    State,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlReadRequest {
    pub node_id: String,
    pub target: RuntimeControlReadTarget,
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlReadResponse {
    pub ok: bool,
    pub address: RuntimeControlReadRequest,
    pub value: Option<Value>,
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
        Self {
            values,
            channels: BTreeMap::new(),
        }
    }

    pub fn value_for_node(&self, node_id: &str) -> Option<&ControlValue> {
        self.values.get(node_id)
    }

    pub fn output_value_for_node(&self, node: &GraphNode, port_id: &str) -> Option<ControlValue> {
        if port_id != "value" {
            return None;
        }
        if let Some(data_kind) = receive_data_kind(&node.kind) {
            return Some(self.receive_value(node, data_kind));
        }
        self.values.get(&node.id).cloned()
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

        if !supports_runtime_control_events(&node.kind) {
            return RuntimeControlEventResponse::error(format!(
                "node {} ({}) does not support runtime control events",
                node.id, node.kind
            ));
        }

        if let Some(data_kind) = send_data_kind(&node.kind) {
            return self.apply_send_event(node, data_kind, request);
        }

        if is_ui_control_kind(&node.kind) {
            return self.apply_ui_event(node, request);
        }

        let Some(stored) = self.values.get(&node.id).cloned() else {
            return RuntimeControlEventResponse::error(format!(
                "node {} has no runtime control state",
                node.id
            ));
        };

        if !node
            .ports
            .iter()
            .any(|port| port.id == request.port_id && port.direction == PortDirection::Input)
        {
            return RuntimeControlEventResponse::error(format!(
                "node {} does not support runtime control input port {}",
                node.id, request.port_id
            ));
        }

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
                if node.kind == TOGGLE_KIND {
                    let ControlValue::Bool(current) = stored else {
                        return RuntimeControlEventResponse::error(format!(
                            "node {} has non-boolean toggle state",
                            node.id
                        ));
                    };
                    let next = ControlValue::Bool(!current);
                    self.values.insert(node.id.clone(), next.clone());
                    return RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                        node_id: node.id.clone(),
                        port_id: "value".to_owned(),
                        value: next,
                    }]);
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

    fn apply_send_event(
        &mut self,
        node: &GraphNode,
        data_kind: &'static str,
        request: RuntimeControlEventRequest,
    ) -> RuntimeControlEventResponse {
        if request.port_id != "in" {
            return RuntimeControlEventResponse::error(format!(
                "node {} does not support runtime control input port {}",
                node.id, request.port_id
            ));
        }
        if let Err(diagnostic) = ensure_data_kind_value(&request.value, data_kind, &node.id) {
            return RuntimeControlEventResponse::error(diagnostic);
        }
        let Some(key) = channel_key_for_node(node, data_kind) else {
            return RuntimeControlEventResponse::error(format!(
                "send node {} is missing non-empty params.name",
                node.id
            ));
        };
        self.channels.insert(key, request.value.clone());
        RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
            node_id: node.id.clone(),
            port_id: "in".to_owned(),
            value: request.value,
        }])
    }

    fn apply_ui_event(
        &mut self,
        node: &GraphNode,
        request: RuntimeControlEventRequest,
    ) -> RuntimeControlEventResponse {
        match node.kind.as_str() {
            UI_BUTTON_KIND => {
                if request.port_id != "bang" {
                    return unsupported_runtime_control_port(node, &request.port_id);
                }
                if !matches!(request.value, ControlValue::Bang) {
                    return RuntimeControlEventResponse::error(format!(
                        "control input {}.bang expects bang, got {}",
                        node.id,
                        request.value.kind_label()
                    ));
                }
                RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                    node_id: node.id.clone(),
                    port_id: "bang".to_owned(),
                    value: ControlValue::Bang,
                }])
            }
            UI_SLIDER_F32_KIND => {
                if request.port_id != "value" {
                    return unsupported_runtime_control_port(node, &request.port_id);
                }
                let ControlValue::F32(_) = request.value else {
                    return RuntimeControlEventResponse::error(format!(
                        "control input {}.value expects f32, got {}",
                        node.id,
                        request.value.kind_label()
                    ));
                };
                self.values.insert(node.id.clone(), request.value.clone());
                RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                    node_id: node.id.clone(),
                    port_id: "value".to_owned(),
                    value: request.value,
                }])
            }
            UI_TOGGLE_KIND => {
                if request.port_id != "value" {
                    return unsupported_runtime_control_port(node, &request.port_id);
                }
                let next = match request.value {
                    ControlValue::Bang => {
                        let stored = self.values.get(&node.id).cloned().ok_or_else(|| {
                            format!("node {} has no runtime control state", node.id)
                        });
                        let Ok(ControlValue::Bool(current)) = stored else {
                            return RuntimeControlEventResponse::error(format!(
                                "node {} has non-boolean toggle state",
                                node.id
                            ));
                        };
                        ControlValue::Bool(!current)
                    }
                    ControlValue::Bool(value) => ControlValue::Bool(value),
                    value => {
                        return RuntimeControlEventResponse::error(format!(
                            "control input {}.value expects bool or bang, got {}",
                            node.id,
                            value.kind_label()
                        ));
                    }
                };
                self.values.insert(node.id.clone(), next.clone());
                RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                    node_id: node.id.clone(),
                    port_id: "value".to_owned(),
                    value: next,
                }])
            }
            _ => RuntimeControlEventResponse::error(format!(
                "node {} ({}) does not support runtime control events",
                node.id, node.kind
            )),
        }
    }

    fn receive_value(&self, node: &GraphNode, data_kind: &'static str) -> ControlValue {
        channel_key_for_node(node, data_kind)
            .and_then(|key| self.channels.get(&key).cloned())
            .unwrap_or_else(|| default_for_data_kind(node, data_kind))
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
        VALUE_F32_KIND
            | VALUE_I32_KIND
            | VALUE_BOOL_KIND
            | COLOR_RGBA_KIND
            | STRING_KIND
            | TOGGLE_KIND
            | MESSAGE_KIND
    )
}

pub fn supports_runtime_control_events(kind: &str) -> bool {
    is_control_value_kind(kind) || send_data_kind(kind).is_some() || is_ui_control_kind(kind)
}

fn is_ui_control_kind(kind: &str) -> bool {
    matches!(kind, UI_BUTTON_KIND | UI_SLIDER_F32_KIND | UI_TOGGLE_KIND)
}

impl RuntimeControlReadResponse {
    pub fn ok(address: RuntimeControlReadRequest, value: Value) -> Self {
        Self {
            ok: true,
            address,
            value: Some(value),
            diagnostics: Vec::new(),
        }
    }

    pub fn error(address: RuntimeControlReadRequest, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            address,
            value: None,
            diagnostics: vec![RuntimeDiagnostic::error(message)],
        }
    }
}

pub fn read_graph_param(node: &GraphNode, param_id: &str) -> Option<Value> {
    node.params
        .get(param_id)
        .cloned()
        .map(|value| json!({ "type": "json", "value": value }))
}

pub fn read_graph_port(node: &GraphNode, port_id: &str) -> Option<Value> {
    node.ports
        .iter()
        .find(|port| port.id == port_id)
        .and_then(|port| serde_json::to_value(port).ok())
        .map(|value| json!({ "type": "json", "value": value }))
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

fn unsupported_runtime_control_port(
    node: &GraphNode,
    port_id: &str,
) -> RuntimeControlEventResponse {
    RuntimeControlEventResponse::error(format!(
        "node {} does not support runtime control input port {}",
        node.id, port_id
    ))
}

fn send_data_kind(kind: &str) -> Option<&'static str> {
    match kind {
        SEND_F32_KIND => Some("number.f32"),
        SEND_I32_KIND => Some("number.i32"),
        SEND_BOOL_KIND => Some("boolean"),
        SEND_RGBA_KIND => Some("color.rgba"),
        _ => None,
    }
}

fn receive_data_kind(kind: &str) -> Option<&'static str> {
    match kind {
        RECEIVE_F32_KIND => Some("number.f32"),
        RECEIVE_I32_KIND => Some("number.i32"),
        RECEIVE_BOOL_KIND => Some("boolean"),
        RECEIVE_RGBA_KIND => Some("color.rgba"),
        _ => None,
    }
}

fn channel_key_for_node(node: &GraphNode, data_kind: &'static str) -> Option<String> {
    let name = node.params.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(format!("{data_kind}:{name}"))
}

fn default_for_data_kind(node: &GraphNode, data_kind: &'static str) -> ControlValue {
    let value = node.params.get("default");
    match data_kind {
        "number.f32" => ControlValue::F32(value.and_then(Value::as_f64).unwrap_or(0.0)),
        "number.i32" => ControlValue::I32(value.and_then(Value::as_i64).unwrap_or(0)),
        "boolean" => ControlValue::Bool(value.and_then(Value::as_bool).unwrap_or(false)),
        "color.rgba" => ControlValue::Rgba(
            value
                .and_then(read_rgba_value)
                .unwrap_or([1.0, 1.0, 1.0, 1.0]),
        ),
        _ => ControlValue::Bang,
    }
}

fn ensure_data_kind_value(
    value: &ControlValue,
    data_kind: &'static str,
    node_id: &str,
) -> Result<(), String> {
    let ok = matches!(
        (data_kind, value),
        ("number.f32", ControlValue::F32(_))
            | ("number.i32", ControlValue::I32(_))
            | ("boolean", ControlValue::Bool(_))
            | ("color.rgba", ControlValue::Rgba(_))
    );
    if ok {
        return Ok(());
    }
    Err(format!(
        "control input {node_id} expects {}, got {}",
        data_kind_label(data_kind),
        value.kind_label()
    ))
}

fn data_kind_label(data_kind: &'static str) -> &'static str {
    match data_kind {
        "number.f32" => "f32",
        "number.i32" => "i32",
        "boolean" => "bool",
        "color.rgba" => "rgba",
        _ => data_kind,
    }
}

fn read_rgba_value(value: &Value) -> Option<[f64; 4]> {
    let values = value.as_array()?;
    let [r, g, b, a] = values.as_slice() else {
        return None;
    };
    Some([r.as_f64()?, g.as_f64()?, b.as_f64()?, a.as_f64()?])
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;
    use crate::{DataFlow, DataType, GraphNode, Port, PortActivation};

    #[test]
    fn initializes_control_values_from_graph() {
        let state = ControlState::from_graph(&graph(vec![
            value_node("f32", VALUE_F32_KIND, json!(1.25)),
            value_node("i32", VALUE_I32_KIND, json!(7)),
            value_node("bool", VALUE_BOOL_KIND, json!(true)),
            value_node("rgba", COLOR_RGBA_KIND, json!([0.1, 0.2, 0.3, 1.0])),
            value_node("string", STRING_KIND, json!("ready")),
            value_node("toggle", TOGGLE_KIND, json!(false)),
            value_node("message", MESSAGE_KIND, json!("perform")),
            value_node("slider", UI_SLIDER_F32_KIND, json!(0.75)),
            value_node("ui_toggle", UI_TOGGLE_KIND, json!(true)),
            value_node("other", "core.target", json!(10)),
        ]));

        assert_eq!(state.values.len(), 9);
        assert!(state.channels.is_empty());
        assert_eq!(state.value_for_node("f32"), Some(&ControlValue::F32(1.25)));
        assert_eq!(state.value_for_node("i32"), Some(&ControlValue::I32(7)));
        assert_eq!(
            state.value_for_node("rgba"),
            Some(&ControlValue::Rgba([0.1, 0.2, 0.3, 1.0]))
        );
        assert_eq!(
            state.value_for_node("string"),
            Some(&ControlValue::String("ready".to_owned()))
        );
        assert_eq!(
            state.value_for_node("toggle"),
            Some(&ControlValue::Bool(false))
        );
        assert_eq!(
            state.value_for_node("message"),
            Some(&ControlValue::String("perform".to_owned()))
        );
        assert_eq!(
            state.value_for_node("slider"),
            Some(&ControlValue::F32(0.75))
        );
        assert_eq!(
            state.value_for_node("ui_toggle"),
            Some(&ControlValue::Bool(true))
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
    fn toggle_bang_flips_and_emits() {
        let graph = graph(vec![value_node("toggle_1", TOGGLE_KIND, json!(false))]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "toggle_1".to_owned(),
                port_id: "bang".to_owned(),
                value: ControlValue::Bang,
            },
            &graph,
        );

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![RuntimeControlEmission {
                node_id: "toggle_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::Bool(true)
            }]
        );
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::Bool(true))
        );
    }

    #[test]
    fn string_and_message_controls_emit_strings() {
        let graph = graph(vec![
            value_node("string_1", STRING_KIND, json!("ready")),
            value_node("message_1", MESSAGE_KIND, json!("perform")),
        ]);
        let mut state = ControlState::from_graph(&graph);

        let string_response = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "string_1".to_owned(),
                port_id: "in".to_owned(),
                value: ControlValue::String("running".to_owned()),
            },
            &graph,
        );
        assert!(string_response.ok);
        assert_eq!(
            string_response.emitted[0].value,
            ControlValue::String("running".to_owned())
        );

        let message_response = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "message_1".to_owned(),
                port_id: "bang".to_owned(),
                value: ControlValue::Bang,
            },
            &graph,
        );
        assert!(message_response.ok);
        assert_eq!(
            message_response.emitted,
            vec![RuntimeControlEmission {
                node_id: "message_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::String("perform".to_owned())
            }]
        );
    }

    #[test]
    fn send_updates_typed_channel() {
        let graph = graph(vec![send_node(
            "send_1",
            SEND_F32_KIND,
            "number.f32",
            "speed",
        )]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "send_1".to_owned(),
                port_id: "in".to_owned(),
                value: ControlValue::F32(1.5),
            },
            &graph,
        );

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![RuntimeControlEmission {
                node_id: "send_1".to_owned(),
                port_id: "in".to_owned(),
                value: ControlValue::F32(1.5)
            }]
        );
        assert_eq!(
            state.channels.get("number.f32:speed"),
            Some(&ControlValue::F32(1.5))
        );

        let wrong_type = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "send_1".to_owned(),
                port_id: "in".to_owned(),
                value: ControlValue::Bool(true),
            },
            &graph,
        );
        assert!(!wrong_type.ok);
        assert_eq!(
            state.channels.get("number.f32:speed"),
            Some(&ControlValue::F32(1.5))
        );
    }

    #[test]
    fn send_rejects_missing_name_wrong_port_and_routes_all_typed_channels() {
        let mut missing_name = send_node("send_missing", SEND_F32_KIND, "number.f32", " ");
        missing_name.params.insert("name".to_owned(), json!(" "));
        let graph = graph(vec![
            send_node("send_i32", SEND_I32_KIND, "number.i32", "iterations"),
            send_node("send_bool", SEND_BOOL_KIND, "boolean", "enabled"),
            send_node("send_rgba", SEND_RGBA_KIND, "color.rgba", "tint"),
            missing_name,
        ]);
        let mut state = ControlState::from_graph(&graph);

        let wrong_port = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "send_i32".to_owned(),
                port_id: "set".to_owned(),
                value: ControlValue::I32(8),
            },
            &graph,
        );
        assert!(!wrong_port.ok);
        assert!(state.channels.is_empty());

        for (node_id, value, channel) in [
            (
                "send_i32",
                ControlValue::I32(8),
                ("number.i32:iterations", ControlValue::I32(8)),
            ),
            (
                "send_bool",
                ControlValue::Bool(true),
                ("boolean:enabled", ControlValue::Bool(true)),
            ),
            (
                "send_rgba",
                ControlValue::Rgba([0.2, 0.4, 0.6, 1.0]),
                ("color.rgba:tint", ControlValue::Rgba([0.2, 0.4, 0.6, 1.0])),
            ),
        ] {
            let response = state.apply_event(
                RuntimeControlEventRequest {
                    node_id: node_id.to_owned(),
                    port_id: "in".to_owned(),
                    value,
                },
                &graph,
            );
            assert!(response.ok);
            assert_eq!(state.channels.get(channel.0), Some(&channel.1));
        }

        let missing_name = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "send_missing".to_owned(),
                port_id: "in".to_owned(),
                value: ControlValue::F32(1.0),
            },
            &graph,
        );
        assert!(!missing_name.ok);
        assert!(
            missing_name.diagnostics[0]
                .message
                .contains("missing non-empty params.name")
        );
    }

    #[test]
    fn receive_resolves_channel_or_default() {
        let graph = graph(vec![receive_node(
            "receive_1",
            RECEIVE_F32_KIND,
            "number.f32",
            "speed",
            json!(0.25),
        )]);
        let mut state = ControlState::from_graph(&graph);
        let node = &graph.nodes[0];

        assert_eq!(
            state.output_value_for_node(node, "value"),
            Some(ControlValue::F32(0.25))
        );

        state
            .channels
            .insert("number.f32:speed".to_owned(), ControlValue::F32(1.75));

        assert_eq!(
            state.output_value_for_node(node, "value"),
            Some(ControlValue::F32(1.75))
        );
    }

    #[test]
    fn receive_resolves_all_default_types_and_non_value_ports() {
        let graph = graph(vec![
            receive_node(
                "receive_i32",
                RECEIVE_I32_KIND,
                "number.i32",
                "iterations",
                json!(8),
            ),
            receive_node(
                "receive_bool",
                RECEIVE_BOOL_KIND,
                "boolean",
                "enabled",
                json!(true),
            ),
            receive_node(
                "receive_rgba",
                RECEIVE_RGBA_KIND,
                "color.rgba",
                "tint",
                json!([0.2, 0.4, 0.6, 1.0]),
            ),
            receive_node(
                "receive_bad_rgba",
                RECEIVE_RGBA_KIND,
                "color.rgba",
                "",
                json!([0.2, 0.4]),
            ),
        ]);
        let mut state = ControlState::from_graph(&graph);

        assert_eq!(
            state.output_value_for_node(&graph.nodes[0], "value"),
            Some(ControlValue::I32(8))
        );
        assert_eq!(
            state.output_value_for_node(&graph.nodes[1], "value"),
            Some(ControlValue::Bool(true))
        );
        assert_eq!(
            state.output_value_for_node(&graph.nodes[2], "value"),
            Some(ControlValue::Rgba([0.2, 0.4, 0.6, 1.0]))
        );
        assert_eq!(
            state.output_value_for_node(&graph.nodes[3], "value"),
            Some(ControlValue::Rgba([1.0, 1.0, 1.0, 1.0]))
        );
        assert_eq!(state.output_value_for_node(&graph.nodes[0], "bang"), None);

        state
            .channels
            .insert("number.i32:iterations".to_owned(), ControlValue::I32(12));
        state
            .channels
            .insert("boolean:enabled".to_owned(), ControlValue::Bool(false));
        state.channels.insert(
            "color.rgba:tint".to_owned(),
            ControlValue::Rgba([1.0, 0.0, 0.0, 1.0]),
        );

        assert_eq!(
            state.output_value_for_node(&graph.nodes[0], "value"),
            Some(ControlValue::I32(12))
        );
        assert_eq!(
            state.output_value_for_node(&graph.nodes[1], "value"),
            Some(ControlValue::Bool(false))
        );
        assert_eq!(
            state.output_value_for_node(&graph.nodes[2], "value"),
            Some(ControlValue::Rgba([1.0, 0.0, 0.0, 1.0]))
        );
    }

    #[test]
    fn ui_panel_controls_emit_runtime_values() {
        let graph = graph(vec![
            value_node("slider_1", UI_SLIDER_F32_KIND, json!(0.5)),
            value_node("toggle_1", UI_TOGGLE_KIND, json!(false)),
            ui_button_node("button_1"),
        ]);
        let mut state = ControlState::from_graph(&graph);

        let slider = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "slider_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::F32(1.25),
            },
            &graph,
        );
        assert!(slider.ok);
        assert_eq!(slider.emitted[0].value, ControlValue::F32(1.25));
        assert_eq!(
            state.value_for_node("slider_1"),
            Some(&ControlValue::F32(1.25))
        );

        let toggle = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "toggle_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::Bang,
            },
            &graph,
        );
        assert!(toggle.ok);
        assert_eq!(toggle.emitted[0].value, ControlValue::Bool(true));
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::Bool(true))
        );

        let button = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "button_1".to_owned(),
                port_id: "bang".to_owned(),
                value: ControlValue::Bang,
            },
            &graph,
        );
        assert!(button.ok);
        assert_eq!(button.emitted[0].value, ControlValue::Bang);
    }

    #[test]
    fn ui_panel_controls_reject_wrong_ports_and_types() {
        let graph = graph(vec![
            value_node("slider_1", UI_SLIDER_F32_KIND, json!(0.5)),
            value_node("toggle_1", UI_TOGGLE_KIND, json!(false)),
            ui_button_node("button_1"),
        ]);
        let mut state = ControlState::from_graph(&graph);

        for request in [
            RuntimeControlEventRequest {
                node_id: "button_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::Bang,
            },
            RuntimeControlEventRequest {
                node_id: "button_1".to_owned(),
                port_id: "bang".to_owned(),
                value: ControlValue::Bool(true),
            },
            RuntimeControlEventRequest {
                node_id: "slider_1".to_owned(),
                port_id: "set".to_owned(),
                value: ControlValue::F32(1.0),
            },
            RuntimeControlEventRequest {
                node_id: "slider_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::Bool(true),
            },
            RuntimeControlEventRequest {
                node_id: "toggle_1".to_owned(),
                port_id: "set".to_owned(),
                value: ControlValue::Bool(true),
            },
            RuntimeControlEventRequest {
                node_id: "toggle_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::F32(1.0),
            },
        ] {
            let response = state.apply_event(request, &graph);
            assert!(!response.ok);
            assert!(response.emitted.is_empty());
        }

        let bool_toggle = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "toggle_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::Bool(true),
            },
            &graph,
        );
        assert!(bool_toggle.ok);
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::Bool(true))
        );

        state.values.remove("toggle_1");
        let missing_state = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "toggle_1".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::Bang,
            },
            &graph,
        );
        assert!(!missing_state.ok);
        assert!(
            missing_state.diagnostics[0]
                .message
                .contains("non-boolean toggle state")
        );
    }

    #[test]
    fn control_state_response_serializes_values_and_channels() {
        let mut values = BTreeMap::new();
        values.insert("slider_1".to_owned(), ControlValue::F32(0.5));
        let mut channels = BTreeMap::new();
        channels.insert("number.f32:speed".to_owned(), ControlValue::F32(1.5));

        let response = RuntimeControlStateResponse {
            ok: true,
            values,
            channels,
            diagnostics: Vec::new(),
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "ok": true,
                "values": {
                    "slider_1": { "type": "f32", "value": 0.5 }
                },
                "channels": {
                    "number.f32:speed": { "type": "f32", "value": 1.5 }
                },
                "diagnostics": []
            })
        );
    }

    #[test]
    fn helper_fallbacks_and_read_responses_are_covered() {
        let node = value_node("value_1", VALUE_F32_KIND, json!(0.5));
        let address = RuntimeControlReadRequest {
            node_id: "value_1".to_owned(),
            target: RuntimeControlReadTarget::Param,
            id: "value".to_owned(),
        };

        assert!(supports_runtime_control_events(SEND_RGBA_KIND));
        assert!(supports_runtime_control_events(UI_BUTTON_KIND));
        assert!(!supports_runtime_control_events("core.target"));
        assert_eq!(data_kind_label("custom.kind"), "custom.kind");
        assert_eq!(
            default_for_data_kind(&node, "custom.kind"),
            ControlValue::Bang
        );
        assert_eq!(
            ControlState::from_graph(&graph(vec![node.clone()]))
                .output_value_for_node(&node, "value",),
            Some(ControlValue::F32(0.5))
        );

        assert_eq!(
            RuntimeControlReadResponse::ok(address.clone(), json!({ "type": "json" })).value,
            Some(json!({ "type": "json" }))
        );
        assert!(!RuntimeControlReadResponse::error(address, "missing value").ok);
    }

    #[test]
    fn ui_event_defensive_unknown_kind_branch_is_covered() {
        let node = value_node("custom_ui", "ui.custom", json!(null));
        let mut state = ControlState::default();

        let response = state.apply_ui_event(
            &node,
            RuntimeControlEventRequest {
                node_id: "custom_ui".to_owned(),
                port_id: "value".to_owned(),
                value: ControlValue::Bang,
            },
        );

        assert!(!response.ok);
        assert!(
            response.diagnostics[0]
                .message
                .contains("does not support runtime control events")
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
    fn rejects_corrupt_toggle_state_and_existing_unsupported_input_port() {
        let mut graph = graph(vec![value_node("toggle_1", TOGGLE_KIND, json!(false))]);
        graph.nodes[0].ports.push(port(
            "other",
            PortDirection::Input,
            DataFlow::Event,
            "event.bang",
            Some(PortActivation::Trigger),
        ));
        let mut state = ControlState::from_graph(&graph);
        state.values.insert(
            "toggle_1".to_owned(),
            ControlValue::String("not-bool".to_owned()),
        );

        let corrupt = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "toggle_1".to_owned(),
                port_id: "bang".to_owned(),
                value: ControlValue::Bang,
            },
            &graph,
        );
        assert!(!corrupt.ok);
        assert!(
            corrupt.diagnostics[0]
                .message
                .contains("non-boolean toggle state")
        );

        state
            .values
            .insert("toggle_1".to_owned(), ControlValue::Bool(false));
        let unsupported = state.apply_event(
            RuntimeControlEventRequest {
                node_id: "toggle_1".to_owned(),
                port_id: "other".to_owned(),
                value: ControlValue::Bang,
            },
            &graph,
        );
        assert!(!unsupported.ok);
        assert!(
            unsupported.diagnostics[0]
                .message
                .contains("does not support runtime control input port other")
        );
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
        let ports = match kind {
            VALUE_F32_KIND => stored_value_ports("number.f32"),
            VALUE_I32_KIND => stored_value_ports("number.i32"),
            VALUE_BOOL_KIND | TOGGLE_KIND => stored_value_ports("boolean"),
            COLOR_RGBA_KIND => stored_value_ports("color.rgba"),
            STRING_KIND => stored_value_ports("string"),
            MESSAGE_KIND => message_ports(),
            UI_SLIDER_F32_KIND => vec![port(
                "value",
                PortDirection::Output,
                DataFlow::Value,
                "number.f32",
                None,
            )],
            UI_TOGGLE_KIND => vec![port(
                "value",
                PortDirection::Output,
                DataFlow::Value,
                "boolean",
                None,
            )],
            _ => Vec::new(),
        };
        GraphNode {
            id: id.to_owned(),
            kind: kind.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports,
        }
    }

    fn send_node(id: &str, kind: &str, data_kind: &str, name: &str) -> GraphNode {
        let mut params = Map::new();
        params.insert("name".to_owned(), json!(name));
        GraphNode {
            id: id.to_owned(),
            kind: kind.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![port(
                "in",
                PortDirection::Input,
                DataFlow::Value,
                data_kind,
                Some(PortActivation::Trigger),
            )],
        }
    }

    fn receive_node(
        id: &str,
        kind: &str,
        data_kind: &str,
        name: &str,
        default: serde_json::Value,
    ) -> GraphNode {
        let mut params = Map::new();
        params.insert("name".to_owned(), json!(name));
        params.insert("default".to_owned(), default);
        GraphNode {
            id: id.to_owned(),
            kind: kind.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![
                port(
                    "bang",
                    PortDirection::Input,
                    DataFlow::Event,
                    "event.bang",
                    Some(PortActivation::Trigger),
                ),
                port(
                    "value",
                    PortDirection::Output,
                    DataFlow::Value,
                    data_kind,
                    None,
                ),
            ],
        }
    }

    fn ui_button_node(id: &str) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            kind: UI_BUTTON_KIND.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params: Map::new(),
            ports: vec![port(
                "bang",
                PortDirection::Output,
                DataFlow::Event,
                "event.bang",
                None,
            )],
        }
    }

    fn stored_value_ports(data_kind: &str) -> Vec<Port> {
        vec![
            port(
                "in",
                PortDirection::Input,
                DataFlow::Value,
                data_kind,
                Some(PortActivation::Trigger),
            ),
            port(
                "set",
                PortDirection::Input,
                DataFlow::Value,
                data_kind,
                Some(PortActivation::Latched),
            ),
            port(
                "bang",
                PortDirection::Input,
                DataFlow::Event,
                "event.bang",
                Some(PortActivation::Trigger),
            ),
            port(
                "value",
                PortDirection::Output,
                DataFlow::Value,
                data_kind,
                None,
            ),
        ]
    }

    fn message_ports() -> Vec<Port> {
        vec![
            port(
                "bang",
                PortDirection::Input,
                DataFlow::Event,
                "event.bang",
                Some(PortActivation::Trigger),
            ),
            port(
                "value",
                PortDirection::Output,
                DataFlow::Value,
                "string",
                None,
            ),
        ]
    }

    fn port(
        id: &str,
        direction: PortDirection,
        flow: DataFlow,
        data_kind: &str,
        activation: Option<PortActivation>,
    ) -> Port {
        Port {
            id: id.to_owned(),
            direction,
            label: None,
            data_type: DataType {
                flow,
                data_kind: data_kind.to_owned(),
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
            },
            required: None,
            default_value: None,
            activation,
        }
    }
}
