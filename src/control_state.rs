use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ControlMessage, ControlValue, GraphDocument, GraphNode, PortDirection, RuntimeDiagnostic,
    control_value::{
        BOOL_KIND, COLOR_KIND, FLOAT_KIND, INT_KIND, MESSAGE_KIND, PANEL_KIND, STRING_KIND,
        TOGGLE_KIND, UI_BUTTON_KIND, UI_SLIDER_FLOAT_KIND, UI_TOGGLE_KIND, UINT_KIND,
    },
    convert_control_value_to_stored,
};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    pub values: BTreeMap<String, ControlValue>,
    pub channels: BTreeMap<String, ControlMessage>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlEventRequest {
    pub node_id: String,
    pub port_id: String,
    pub message: ControlMessage,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlEmission {
    pub node_id: String,
    pub port_id: String,
    pub message: ControlMessage,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlEventResponse {
    pub ok: bool,
    pub changed: bool,
    pub control_revision: Option<u64>,
    pub emitted: Vec<RuntimeControlEmission>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlStateResponse {
    pub ok: bool,
    pub control_revision: u64,
    pub values: BTreeMap<String, ControlValue>,
    pub channels: BTreeMap<String, ControlMessage>,
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

impl RuntimeControlEventRequest {
    fn control_message(&self) -> ControlMessage {
        self.message.clone()
    }
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
        self.values.get(&node.id).cloned()
    }

    pub fn apply_event(
        &mut self,
        request: RuntimeControlEventRequest,
        graph: &GraphDocument,
    ) -> RuntimeControlEventResponse {
        let response = self.apply_event_direct(request, graph);
        if !response.ok {
            return response;
        }

        self.propagate_emissions(response, graph)
    }

    fn apply_event_direct(
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

        if is_ui_control_kind(&node.kind) {
            return self.apply_ui_event(node, request);
        }

        let message = request.control_message();

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
                if matches!(node.kind.as_str(), MESSAGE_KIND | PANEL_KIND) {
                    let next = set_message_text(&message);
                    self.values
                        .insert(node.id.clone(), ControlValue::string(next));
                    return RuntimeControlEventResponse::ok(Vec::new());
                }
                let Some(next) = value_from_message(&message, &stored) else {
                    return RuntimeControlEventResponse::error(type_error_from_message(
                        &message, &stored, &node.id,
                    ));
                };
                self.values.insert(node.id.clone(), next);
                RuntimeControlEventResponse::ok(Vec::new())
            }
            "in" => {
                if node.kind == MESSAGE_KIND {
                    if let Some(next) = silent_set_message(&message) {
                        self.values
                            .insert(node.id.clone(), ControlValue::string(next));
                        return RuntimeControlEventResponse::ok(Vec::new());
                    }
                    return RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                        node_id: node.id.clone(),
                        port_id: "value".to_owned(),
                        message: message_from_message_node_state(&stored),
                    }]);
                }
                if node.kind == TOGGLE_KIND {
                    return self.apply_toggle_event(node, "in", message, stored);
                }
                let Some(next) = value_from_message(&message, &stored) else {
                    return RuntimeControlEventResponse::error(type_error_from_message(
                        &message, &stored, &node.id,
                    ));
                };
                self.values.insert(node.id.clone(), next.clone());
                RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                    node_id: node.id.clone(),
                    port_id: "value".to_owned(),
                    message: ControlMessage::from_value(next),
                }])
            }
            "bang" => {
                if !is_bang_message(&message) {
                    return RuntimeControlEventResponse::error(format!(
                        "control input {}.bang expects bang, got {}",
                        node.id, message.selector
                    ));
                }
                if node.kind == TOGGLE_KIND {
                    return self.apply_toggle_event(node, "bang", message, stored);
                }
                RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                    node_id: node.id.clone(),
                    port_id: "value".to_owned(),
                    message: ControlMessage::from_value(stored),
                }])
            }
            port => RuntimeControlEventResponse::error(format!(
                "node {} does not support runtime control input port {}",
                node.id, port
            )),
        }
    }

    fn propagate_emissions(
        &mut self,
        mut response: RuntimeControlEventResponse,
        graph: &GraphDocument,
    ) -> RuntimeControlEventResponse {
        let mut queue = response.emitted.clone();
        let mut visited_edges = 0usize;
        while let Some(emission) = queue.pop() {
            self.publish_object_channel(&emission, graph);
            visited_edges += 1;
            if visited_edges > graph.edges.len().saturating_mul(2).max(32) {
                return RuntimeControlEventResponse::error(
                    "control event propagation exceeded the v0 runtime safety limit",
                );
            }

            for edge in graph.edges.iter().filter(|edge| {
                edge.from.node == emission.node_id && edge.from.port == emission.port_id
            }) {
                let Some(target_node) = graph.nodes.iter().find(|node| node.id == edge.to.node)
                else {
                    continue;
                };
                if !supports_runtime_control_events(&target_node.kind) {
                    continue;
                }
                let target_response = self.apply_event_direct(
                    RuntimeControlEventRequest {
                        node_id: target_node.id.clone(),
                        port_id: edge.to.port.clone(),
                        message: emission.message.clone(),
                    },
                    graph,
                );
                if !target_response.ok {
                    return target_response;
                }
                for target_emission in target_response.emitted {
                    queue.push(target_emission.clone());
                    response.emitted.push(target_emission);
                }
            }
        }

        response
    }

    fn publish_object_channel(&mut self, emission: &RuntimeControlEmission, graph: &GraphDocument) {
        let Some(source_node) = graph.nodes.iter().find(|node| node.id == emission.node_id) else {
            return;
        };
        let data_kind = data_kind_for_control_message(&emission.message);
        let Some(channel_name) = read_named_param(source_node, "sendName") else {
            return;
        };
        let key = format!("{data_kind}:{channel_name}");
        self.channels.insert(key, emission.message.clone());
        self.apply_receive_name_updates(
            data_kind,
            &channel_name,
            &emission.message,
            &emission.node_id,
            graph,
        );
    }

    fn apply_receive_name_updates(
        &mut self,
        data_kind: &'static str,
        channel_name: &str,
        message: &ControlMessage,
        source_node_id: &str,
        graph: &GraphDocument,
    ) {
        for node in graph.nodes.iter().filter(|node| node.id != source_node_id) {
            if read_named_param(node, "receiveName").as_deref() != Some(channel_name) {
                continue;
            }
            if !object_accepts_data_kind(node, data_kind) {
                continue;
            }
            if let Some(stored) = self.values.get(&node.id).cloned()
                && let Some(value) = value_from_message(message, &stored)
            {
                self.values.insert(node.id.clone(), value);
            }
        }
    }

    fn apply_ui_event(
        &mut self,
        node: &GraphNode,
        request: RuntimeControlEventRequest,
    ) -> RuntimeControlEventResponse {
        match node.kind.as_str() {
            UI_BUTTON_KIND => {
                if request.port_id != "in" && request.port_id != "bang" {
                    return unsupported_runtime_control_port(node, &request.port_id);
                }
                RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                    node_id: node.id.clone(),
                    port_id: "bang".to_owned(),
                    message: ControlMessage::bang(),
                }])
            }
            UI_SLIDER_FLOAT_KIND => self.apply_slider_event(node, request),
            UI_TOGGLE_KIND => {
                let Some(stored) = self.values.get(&node.id).cloned() else {
                    return RuntimeControlEventResponse::error(format!(
                        "node {} has no runtime control state",
                        node.id
                    ));
                };
                let message = request.control_message();
                self.apply_toggle_event(node, &request.port_id, message, stored)
            }
            _ => RuntimeControlEventResponse::error(format!(
                "node {} ({}) does not support runtime control events",
                node.id, node.kind
            )),
        }
    }

    fn apply_slider_event(
        &mut self,
        node: &GraphNode,
        request: RuntimeControlEventRequest,
    ) -> RuntimeControlEventResponse {
        match request.port_id.as_str() {
            "set" => {
                let message = request.control_message();
                let Some(value @ ControlValue::Float { .. }) =
                    value_from_message(&message, &ControlValue::float(0.0))
                else {
                    return RuntimeControlEventResponse::error(format!(
                        "control input {}.set expects number.float, got {}",
                        node.id, message.selector
                    ));
                };
                self.values.insert(node.id.clone(), value);
                RuntimeControlEventResponse::ok(Vec::new())
            }
            "in" | "value" => {
                let message = request.control_message();
                let Some(value @ ControlValue::Float { .. }) =
                    value_from_message(&message, &ControlValue::float(0.0))
                else {
                    return RuntimeControlEventResponse::error(format!(
                        "control input {}.{} expects number.float, got {}",
                        node.id, request.port_id, message.selector
                    ));
                };
                self.values.insert(node.id.clone(), value.clone());
                RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                    node_id: node.id.clone(),
                    port_id: "value".to_owned(),
                    message: ControlMessage::from_value(value),
                }])
            }
            "bang" => {
                let message = request.control_message();
                if !is_bang_message(&message) {
                    return RuntimeControlEventResponse::error(format!(
                        "control input {}.bang expects bang, got {}",
                        node.id, message.selector
                    ));
                }
                let Some(value) = self.values.get(&node.id).cloned() else {
                    return RuntimeControlEventResponse::error(format!(
                        "node {} has no runtime control state",
                        node.id
                    ));
                };
                RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                    node_id: node.id.clone(),
                    port_id: "value".to_owned(),
                    message: ControlMessage::from_value(value),
                }])
            }
            _ => unsupported_runtime_control_port(node, &request.port_id),
        }
    }

    fn apply_toggle_event(
        &mut self,
        node: &GraphNode,
        port_id: &str,
        message: ControlMessage,
        stored: ControlValue,
    ) -> RuntimeControlEventResponse {
        let ControlValue::Bool { value: current } = stored else {
            return RuntimeControlEventResponse::error(format!(
                "node {} has non-boolean toggle state",
                node.id
            ));
        };
        let silent = port_id == "set" || message.selector == "set";
        let Some(next_bool) = coerce_toggle_input(&message, current) else {
            return RuntimeControlEventResponse::error(format!(
                "control input {}.{} expects bang, bool, 0/1, or on/off",
                node.id, port_id
            ));
        };
        let next = ControlValue::bool(next_bool);
        self.values.insert(node.id.clone(), next.clone());
        if silent {
            RuntimeControlEventResponse::ok(Vec::new())
        } else {
            RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                node_id: node.id.clone(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(next),
            }])
        }
    }
}

impl RuntimeControlEventResponse {
    fn ok(emitted: Vec<RuntimeControlEmission>) -> Self {
        Self {
            ok: true,
            changed: false,
            control_revision: None,
            emitted,
            diagnostics: Vec::new(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            changed: false,
            control_revision: None,
            emitted: Vec::new(),
            diagnostics: vec![RuntimeDiagnostic::error(message)],
        }
    }

    pub(crate) fn with_runtime_metadata(mut self, changed: bool, control_revision: u64) -> Self {
        self.changed = changed;
        self.control_revision = Some(control_revision);
        self
    }
}

pub fn is_control_value_kind(kind: &str) -> bool {
    matches!(
        kind,
        FLOAT_KIND
            | INT_KIND
            | UINT_KIND
            | BOOL_KIND
            | COLOR_KIND
            | STRING_KIND
            | TOGGLE_KIND
            | MESSAGE_KIND
            | PANEL_KIND
    )
}

pub fn supports_runtime_control_events(kind: &str) -> bool {
    is_control_value_kind(kind) || is_ui_control_kind(kind)
}

fn is_ui_control_kind(kind: &str) -> bool {
    matches!(kind, UI_BUTTON_KIND | UI_SLIDER_FLOAT_KIND | UI_TOGGLE_KIND)
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

fn unsupported_runtime_control_port(
    node: &GraphNode,
    port_id: &str,
) -> RuntimeControlEventResponse {
    RuntimeControlEventResponse::error(format!(
        "node {} does not support runtime control input port {}",
        node.id, port_id
    ))
}

fn read_named_param(node: &GraphNode, key: &str) -> Option<String> {
    let value = node.params.get(key)?.as_str()?.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.to_owned())
}

fn data_kind_for_control_message(message: &ControlMessage) -> &'static str {
    if is_bang_message(message) {
        return "event.bang";
    }
    match message.first_atom() {
        Some(value) => data_kind_for_control_value(value),
        None => "message.any",
    }
}

fn data_kind_for_control_value(value: &ControlValue) -> &'static str {
    match value {
        ControlValue::Float { .. } => "number.float",
        ControlValue::Int { .. } => "number.int",
        ControlValue::Uint { .. } => "number.uint",
        ControlValue::Bool { .. } => "boolean",
        ControlValue::String { .. } => "string",
        ControlValue::Color { .. } => "color",
    }
}

fn object_accepts_data_kind(node: &GraphNode, data_kind: &'static str) -> bool {
    match node.kind.as_str() {
        FLOAT_KIND | UI_SLIDER_FLOAT_KIND => data_kind == "number.float",
        INT_KIND => data_kind == "number.int",
        UINT_KIND => data_kind == "number.uint",
        BOOL_KIND | TOGGLE_KIND | UI_TOGGLE_KIND => data_kind == "boolean",
        COLOR_KIND => data_kind == "color",
        STRING_KIND | PANEL_KIND => data_kind == "string",
        MESSAGE_KIND => matches!(data_kind, "message.any" | "string" | "event.bang"),
        UI_BUTTON_KIND => data_kind == "event.bang",
        _ => false,
    }
}

fn is_bang_message(message: &ControlMessage) -> bool {
    message.selector == "bang" && message.atoms.is_empty()
}

fn value_from_message(message: &ControlMessage, stored: &ControlValue) -> Option<ControlValue> {
    let atom = message.first_atom();
    match stored {
        ControlValue::Float { .. }
        | ControlValue::Int { .. }
        | ControlValue::Uint { .. }
        | ControlValue::Color { .. } => {
            atom.and_then(|value| convert_control_value_to_stored(value, stored))
        }
        ControlValue::Bool { .. } => coerce_toggle_input(message, false).map(ControlValue::bool),
        ControlValue::String { .. } => {
            if message.selector == "symbol"
                && let Some(ControlValue::String { value }) = atom
            {
                return Some(ControlValue::string(value.clone()));
            }
            Some(ControlValue::string(message.to_text()))
        }
    }
}

fn type_error_from_message(
    message: &ControlMessage,
    stored: &ControlValue,
    node_id: &str,
) -> String {
    format!(
        "control input {node_id} expects {}, got message selector {}",
        stored.kind_label(),
        message.selector
    )
}

fn message_from_message_node_state(stored: &ControlValue) -> ControlMessage {
    match stored {
        ControlValue::String { value } => ControlMessage::parse_text(value),
        value => ControlMessage::from_value(value.clone()),
    }
}

fn set_message_text(message: &ControlMessage) -> String {
    if message.selector == "set" {
        return message
            .atoms
            .iter()
            .map(control_atom_to_text)
            .collect::<Vec<_>>()
            .join(" ");
    }
    message.to_text()
}

fn silent_set_message(message: &ControlMessage) -> Option<String> {
    (message.selector == "set").then(|| set_message_text(message))
}

fn control_atom_to_text(value: &ControlValue) -> String {
    match value {
        ControlValue::Float { value, .. } => value.to_string(),
        ControlValue::Int { value, .. } => value.to_string(),
        ControlValue::Uint { value, .. } => value.to_string(),
        ControlValue::Bool { value } => {
            if *value {
                "on".to_owned()
            } else {
                "off".to_owned()
            }
        }
        ControlValue::String { value } => value.clone(),
        ControlValue::Color { value, .. } => {
            format!("color {} {} {} {}", value[0], value[1], value[2], value[3])
        }
    }
}

fn coerce_toggle_input(message: &ControlMessage, current: bool) -> Option<bool> {
    match message.selector.as_str() {
        "bang" if message.atoms.is_empty() => Some(!current),
        "on" | "true" => Some(true),
        "off" | "false" => Some(false),
        "set" | "float" | "int" | "uint" | "bool" | "symbol" => match message.first_atom()? {
            ControlValue::Bool { value } => Some(*value),
            ControlValue::Int { value, .. } => match value {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            ControlValue::Uint { value, .. } => match value {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            ControlValue::Float { value, .. } if *value == 0.0 => Some(false),
            ControlValue::Float { value, .. } if *value == 1.0 => Some(true),
            ControlValue::String { value } => match value.trim().to_ascii_lowercase().as_str() {
                "0" | "off" | "false" => Some(false),
                "1" | "on" | "true" => Some(true),
                "bang" => Some(!current),
                _ => None,
            },
            _ => None,
        },
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;
    use crate::{DataFlow, DataType, GraphNode, Port, PortActivation};

    fn request(
        node_id: &str,
        port_id: &str,
        message: ControlMessage,
    ) -> RuntimeControlEventRequest {
        RuntimeControlEventRequest {
            node_id: node_id.to_owned(),
            port_id: port_id.to_owned(),
            message,
        }
    }

    fn value_request(
        node_id: &str,
        port_id: &str,
        value: ControlValue,
    ) -> RuntimeControlEventRequest {
        request(node_id, port_id, ControlMessage::from_value(value))
    }

    fn bang_request(node_id: &str, port_id: &str) -> RuntimeControlEventRequest {
        request(node_id, port_id, ControlMessage::bang())
    }

    fn emitted_value(emission: &RuntimeControlEmission) -> Option<ControlValue> {
        emission.message.first_atom().cloned()
    }

    #[test]
    fn initializes_control_values_from_graph() {
        let state = ControlState::from_graph(&graph(vec![
            value_node("f32", FLOAT_KIND, json!(1.25)),
            value_node("i32", INT_KIND, json!(7)),
            value_node("bool", BOOL_KIND, json!(true)),
            value_node("rgba", COLOR_KIND, json!([0.1, 0.2, 0.3, 1.0])),
            value_node("string", STRING_KIND, json!("ready")),
            value_node("toggle", TOGGLE_KIND, json!(false)),
            value_node("message", MESSAGE_KIND, json!("perform")),
            value_node("slider", UI_SLIDER_FLOAT_KIND, json!(0.75)),
            value_node("ui_toggle", UI_TOGGLE_KIND, json!(true)),
            value_node("other", "core.target", json!(10)),
        ]));

        assert_eq!(state.values.len(), 9);
        assert!(state.channels.is_empty());
        assert_eq!(
            state.value_for_node("f32"),
            Some(&ControlValue::float(1.25))
        );
        assert_eq!(state.value_for_node("i32"), Some(&ControlValue::int(7)));
        assert_eq!(
            state.value_for_node("rgba"),
            Some(&ControlValue::color([0.1, 0.2, 0.3, 1.0]))
        );
        assert_eq!(
            state.value_for_node("string"),
            Some(&ControlValue::string("ready".to_owned()))
        );
        assert_eq!(
            state.value_for_node("toggle"),
            Some(&ControlValue::bool(false))
        );
        assert_eq!(
            state.value_for_node("message"),
            Some(&ControlValue::string("perform".to_owned()))
        );
        assert_eq!(
            state.value_for_node("slider"),
            Some(&ControlValue::float(0.75))
        );
        assert_eq!(
            state.value_for_node("ui_toggle"),
            Some(&ControlValue::bool(true))
        );
        assert_eq!(state.value_for_node("other"), None);
    }

    #[test]
    fn set_updates_without_emission() {
        let graph = graph(vec![value_node("value_1", FLOAT_KIND, json!(1.0))]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            value_request("value_1", "set", ControlValue::float(32.0)),
            &graph,
        );

        assert!(response.ok);
        assert!(response.emitted.is_empty());
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::float(32.0))
        );
    }

    #[test]
    fn in_updates_and_emits() {
        let graph = graph(vec![value_node("value_1", INT_KIND, json!(1))]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            value_request("value_1", "in", ControlValue::int(12)),
            &graph,
        );

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![RuntimeControlEmission {
                node_id: "value_1".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::int(12))
            }]
        );
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::int(12))
        );
    }

    #[test]
    fn bang_emits_stored_value_without_update() {
        let graph = graph(vec![value_node("value_1", BOOL_KIND, json!(true))]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(bang_request("value_1", "bang"), &graph);

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![RuntimeControlEmission {
                node_id: "value_1".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::bool(true))
            }]
        );
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::bool(true))
        );
    }

    #[test]
    fn toggle_bang_flips_and_emits() {
        let graph = graph(vec![value_node("toggle_1", TOGGLE_KIND, json!(false))]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(bang_request("toggle_1", "bang"), &graph);

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![RuntimeControlEmission {
                node_id: "toggle_1".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::bool(true))
            }]
        );
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::bool(true))
        );
    }

    #[test]
    fn toggle_accepts_on_off_and_set_messages() {
        let graph = graph(vec![
            value_node("toggle_1", UI_TOGGLE_KIND, json!(false)),
            value_node("core_toggle_1", TOGGLE_KIND, json!(false)),
        ]);
        let mut state = ControlState::from_graph(&graph);

        let on = state.apply_event(
            request("toggle_1", "in", ControlMessage::parse_text("on")),
            &graph,
        );
        assert!(on.ok);
        assert_eq!(
            emitted_value(&on.emitted[0]),
            Some(ControlValue::bool(true))
        );

        let set_off = state.apply_event(
            request("toggle_1", "set", ControlMessage::parse_text("set off")),
            &graph,
        );
        assert!(set_off.ok);
        assert!(set_off.emitted.is_empty());
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::bool(false))
        );

        let core_toggle = state.apply_event(
            request("core_toggle_1", "in", ControlMessage::parse_text("set on")),
            &graph,
        );
        assert!(core_toggle.ok);
        assert!(core_toggle.emitted.is_empty());
        assert_eq!(
            state.value_for_node("core_toggle_1"),
            Some(&ControlValue::bool(true))
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
            value_request("string_1", "in", ControlValue::string("running".to_owned())),
            &graph,
        );
        assert!(string_response.ok);
        assert_eq!(
            emitted_value(&string_response.emitted[0]),
            Some(ControlValue::string("running".to_owned()))
        );

        let message_response = state.apply_event(bang_request("message_1", "bang"), &graph);
        assert!(message_response.ok);
        assert_eq!(
            message_response.emitted,
            vec![RuntimeControlEmission {
                node_id: "message_1".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::parse_text("perform")
            }]
        );

        let set_response = state.apply_event(
            request(
                "message_1",
                "set",
                ControlMessage::parse_text("set updated"),
            ),
            &graph,
        );
        assert!(set_response.ok);
        assert!(set_response.emitted.is_empty());
        assert_eq!(
            state.value_for_node("message_1"),
            Some(&ControlValue::string("updated".to_owned()))
        );

        let silent_in = state.apply_event(
            request("message_1", "in", ControlMessage::parse_text("set queued")),
            &graph,
        );
        assert!(silent_in.ok);
        assert!(silent_in.emitted.is_empty());
        assert_eq!(
            state.value_for_node("message_1"),
            Some(&ControlValue::string("queued".to_owned()))
        );

        let emit_in = state.apply_event(bang_request("message_1", "in"), &graph);
        assert!(emit_in.ok);
        assert_eq!(
            emit_in.emitted,
            vec![RuntimeControlEmission {
                node_id: "message_1".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::parse_text("queued")
            }]
        );
    }

    #[test]
    fn object_send_name_updates_channel_and_receive_name_state() {
        let mut sender = value_node("slider_1", UI_SLIDER_FLOAT_KIND, json!(0.25));
        sender.params.insert("sendName".to_owned(), json!("speed"));
        let mut receiver = value_node("value_1", FLOAT_KIND, json!(0.0));
        receiver
            .params
            .insert("receiveName".to_owned(), json!("speed"));
        let routing_graph = graph(vec![sender, receiver]);
        let mut state = ControlState::from_graph(&routing_graph);

        let response = state.apply_event(
            value_request("slider_1", "in", ControlValue::float(1.25)),
            &routing_graph,
        );

        assert!(response.ok);
        assert_eq!(
            state.channels.get("number.float:speed"),
            Some(&ControlMessage::from_value(ControlValue::float(1.25)))
        );
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::float(1.25))
        );

        let mut bang_sender = ui_button_node("button_1");
        bang_sender
            .params
            .insert("sendName".to_owned(), json!("go"));
        let graph = graph(vec![bang_sender]);
        let mut state = ControlState::from_graph(&graph);
        let bang = state.apply_event(bang_request("button_1", "bang"), &graph);
        assert!(bang.ok);
        assert_eq!(
            state.channels.get("event.bang:go"),
            Some(&ControlMessage::bang())
        );
    }

    #[test]
    fn object_channel_helpers_skip_missing_sources_empty_names_and_mismatched_receivers() {
        let mut sender = value_node("slider_1", UI_SLIDER_FLOAT_KIND, json!(0.25));
        sender.params.insert("sendName".to_owned(), json!("   "));
        let mut wrong_receiver = value_node("bool_1", BOOL_KIND, json!(false));
        wrong_receiver
            .params
            .insert("receiveName".to_owned(), json!("speed"));
        let empty_name_graph = graph(vec![sender, wrong_receiver]);
        let mut state = ControlState::from_graph(&empty_name_graph);

        state.publish_object_channel(
            &RuntimeControlEmission {
                node_id: "missing".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::float(1.0)),
            },
            &empty_name_graph,
        );
        state.publish_object_channel(
            &RuntimeControlEmission {
                node_id: "slider_1".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::float(1.0)),
            },
            &empty_name_graph,
        );
        assert!(state.channels.is_empty());
        assert_eq!(
            state.value_for_node("bool_1"),
            Some(&ControlValue::bool(false))
        );

        let mut sender = value_node("slider_2", UI_SLIDER_FLOAT_KIND, json!(0.25));
        sender.params.insert("sendName".to_owned(), json!("speed"));
        let mut wrong_receiver = value_node("bool_2", BOOL_KIND, json!(false));
        wrong_receiver
            .params
            .insert("receiveName".to_owned(), json!("speed"));
        let mismatched_receiver_graph = graph(vec![sender, wrong_receiver]);
        let mut mismatched_state = ControlState::from_graph(&mismatched_receiver_graph);
        mismatched_state.publish_object_channel(
            &RuntimeControlEmission {
                node_id: "slider_2".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::float(1.0)),
            },
            &mismatched_receiver_graph,
        );
        assert_eq!(
            mismatched_state.value_for_node("bool_2"),
            Some(&ControlValue::bool(false))
        );

        assert_eq!(
            data_kind_for_control_value(&ControlValue::int(1)),
            "number.int"
        );
        assert_eq!(
            data_kind_for_control_value(&ControlValue::uint(1)),
            "number.uint"
        );
        assert_eq!(
            data_kind_for_control_value(&ControlValue::color([1.0, 0.0, 0.0, 1.0])),
            "color"
        );
        assert!(object_accepts_data_kind(
            &value_node("i32_1", INT_KIND, json!(0)),
            "number.int"
        ));
        assert!(object_accepts_data_kind(
            &value_node("u32_1", UINT_KIND, json!(0)),
            "number.uint"
        ));
        assert!(object_accepts_data_kind(
            &value_node("rgba_1", COLOR_KIND, json!([1.0, 0.0, 0.0, 1.0])),
            "color"
        ));
        assert!(object_accepts_data_kind(
            &value_node("message_1", MESSAGE_KIND, json!("go")),
            "string"
        ));
        assert!(object_accepts_data_kind(
            &ui_button_node("button_1"),
            "event.bang"
        ));
        assert!(!object_accepts_data_kind(
            &value_node("target_1", "core.target", json!(null)),
            "string"
        ));
    }

    #[test]
    fn object_set_does_not_update_send_name_channel() {
        let mut node = value_node("value_1", FLOAT_KIND, json!(0.25));
        node.params.insert("sendName".to_owned(), json!("speed"));
        let graph = graph(vec![node]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            value_request("value_1", "set", ControlValue::float(2.0)),
            &graph,
        );

        assert!(response.ok);
        assert!(response.emitted.is_empty());
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::float(2.0))
        );
        assert!(state.channels.is_empty());
    }

    #[test]
    fn object_edges_propagate_to_connected_control_inputs() {
        let mut graph = graph(vec![
            value_node("slider_1", UI_SLIDER_FLOAT_KIND, json!(0.25)),
            value_node("value_1", FLOAT_KIND, json!(0.0)),
            value_node("message_1", MESSAGE_KIND, json!("go")),
            ui_button_node("button_1"),
        ]);
        graph.edges = vec![
            edge("slider_1", "value", "value_1", "in"),
            edge("button_1", "bang", "message_1", "bang"),
        ];
        let mut state = ControlState::from_graph(&graph);

        let slider = state.apply_event(
            value_request("slider_1", "value", ControlValue::float(1.5)),
            &graph,
        );
        assert!(slider.ok);
        assert_eq!(slider.emitted.len(), 2);
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::float(1.5))
        );

        let button = state.apply_event(bang_request("button_1", "in"), &graph);
        assert!(button.ok);
        assert_eq!(
            button.emitted,
            vec![
                RuntimeControlEmission {
                    node_id: "button_1".to_owned(),
                    port_id: "bang".to_owned(),
                    message: ControlMessage::bang()
                },
                RuntimeControlEmission {
                    node_id: "message_1".to_owned(),
                    port_id: "value".to_owned(),
                    message: ControlMessage::parse_text("go")
                }
            ]
        );
    }

    #[test]
    fn object_edge_propagation_ignores_edges_to_missing_targets() {
        let mut graph = graph(vec![value_node(
            "slider_1",
            UI_SLIDER_FLOAT_KIND,
            json!(0.25),
        )]);
        graph.edges = vec![edge("slider_1", "value", "missing", "in")];
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            value_request("slider_1", "value", ControlValue::float(1.5)),
            &graph,
        );

        assert!(response.ok);
        assert_eq!(response.emitted.len(), 1);
        assert!(state.channels.is_empty());
    }

    #[test]
    fn object_edge_propagation_rejects_invalid_target_port() {
        let mut graph = graph(vec![
            value_node("slider_1", UI_SLIDER_FLOAT_KIND, json!(0.25)),
            value_node("value_1", FLOAT_KIND, json!(0.0)),
        ]);
        graph.edges = vec![edge("slider_1", "value", "value_1", "missing")];
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            value_request("slider_1", "value", ControlValue::float(1.5)),
            &graph,
        );

        assert!(!response.ok);
        assert!(response.diagnostics[0].message.contains("port missing"));
        assert!(state.channels.is_empty());
    }

    #[test]
    fn ui_panel_propagation_stops_at_runtime_safety_limit() {
        let mut graph = graph(vec![
            value_node("slider_1", UI_SLIDER_FLOAT_KIND, json!(0.25)),
            value_node("value_1", FLOAT_KIND, json!(0.0)),
        ]);
        graph.edges = vec![
            edge("slider_1", "value", "value_1", "in"),
            edge("value_1", "value", "value_1", "in"),
        ];
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            value_request("slider_1", "value", ControlValue::float(1.5)),
            &graph,
        );

        assert!(!response.ok);
        assert!(
            response.diagnostics[0]
                .message
                .contains("runtime safety limit")
        );
    }

    #[test]
    fn ui_panel_controls_emit_runtime_values() {
        let graph = graph(vec![
            value_node("slider_1", UI_SLIDER_FLOAT_KIND, json!(0.5)),
            value_node("toggle_1", UI_TOGGLE_KIND, json!(false)),
            ui_button_node("button_1"),
        ]);
        let mut state = ControlState::from_graph(&graph);

        let slider = state.apply_event(
            value_request("slider_1", "value", ControlValue::float(1.25)),
            &graph,
        );
        assert!(slider.ok);
        assert_eq!(
            emitted_value(&slider.emitted[0]),
            Some(ControlValue::float(1.25))
        );
        assert_eq!(
            state.value_for_node("slider_1"),
            Some(&ControlValue::float(1.25))
        );

        let toggle = state.apply_event(bang_request("toggle_1", "value"), &graph);
        assert!(toggle.ok);
        assert_eq!(
            emitted_value(&toggle.emitted[0]),
            Some(ControlValue::bool(true))
        );
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::bool(true))
        );

        let button = state.apply_event(bang_request("button_1", "bang"), &graph);
        assert!(button.ok);
        assert_eq!(button.emitted[0].message, ControlMessage::bang());
    }

    #[test]
    fn ui_panel_controls_reject_wrong_ports_and_types() {
        let graph = graph(vec![
            value_node("slider_1", UI_SLIDER_FLOAT_KIND, json!(0.5)),
            value_node("toggle_1", UI_TOGGLE_KIND, json!(false)),
            ui_button_node("button_1"),
        ]);
        let mut state = ControlState::from_graph(&graph);

        for request in [
            bang_request("button_1", "value"),
            value_request("slider_1", "set", ControlValue::bool(true)),
            value_request("slider_1", "value", ControlValue::bool(true)),
            value_request("toggle_1", "value", ControlValue::float(2.0)),
        ] {
            let response = state.apply_event(request, &graph);
            assert!(!response.ok);
            assert!(response.emitted.is_empty());
        }

        let any_button = state.apply_event(
            value_request("button_1", "in", ControlValue::bool(true)),
            &graph,
        );
        assert!(any_button.ok);
        assert_eq!(any_button.emitted[0].message, ControlMessage::bang());

        let slider_set = state.apply_event(
            value_request("slider_1", "set", ControlValue::float(1.0)),
            &graph,
        );
        assert!(slider_set.ok);
        assert!(slider_set.emitted.is_empty());
        assert_eq!(
            state.value_for_node("slider_1"),
            Some(&ControlValue::float(1.0))
        );

        let slider_bang = state.apply_event(bang_request("slider_1", "bang"), &graph);
        assert!(slider_bang.ok);
        assert_eq!(
            emitted_value(&slider_bang.emitted[0]),
            Some(ControlValue::float(1.0))
        );

        let slider_bad_bang = state.apply_event(
            value_request("slider_1", "bang", ControlValue::bool(true)),
            &graph,
        );
        assert!(!slider_bad_bang.ok);

        let slider_other = state.apply_event(
            value_request("slider_1", "other", ControlValue::float(1.0)),
            &graph,
        );
        assert!(!slider_other.ok);

        state.values.remove("slider_1");
        let slider_missing_state = state.apply_event(bang_request("slider_1", "bang"), &graph);
        assert!(!slider_missing_state.ok);
        assert!(
            slider_missing_state.diagnostics[0]
                .message
                .contains("has no runtime control state")
        );

        let bool_toggle = state.apply_event(
            value_request("toggle_1", "value", ControlValue::bool(true)),
            &graph,
        );
        assert!(bool_toggle.ok);
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::bool(true))
        );

        state.values.remove("toggle_1");
        let missing_state = state.apply_event(bang_request("toggle_1", "value"), &graph);
        assert!(!missing_state.ok);
        assert!(
            missing_state.diagnostics[0]
                .message
                .contains("has no runtime control state")
        );
    }

    #[test]
    fn control_state_response_serializes_values_and_channels() {
        let mut values = BTreeMap::new();
        values.insert("slider_1".to_owned(), ControlValue::float(0.5));
        let mut channels = BTreeMap::new();
        channels.insert(
            "number.float:speed".to_owned(),
            ControlMessage::from_value(ControlValue::float(1.5)),
        );

        let response = RuntimeControlStateResponse {
            ok: true,
            control_revision: 7,
            values,
            channels,
            diagnostics: Vec::new(),
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "ok": true,
                "controlRevision": 7,
                "values": {
                    "slider_1": { "type": "float", "representation": "f32", "value": 0.5 }
                },
                "channels": {
                    "number.float:speed": {
                        "selector": "float",
                        "atoms": [{ "type": "float", "representation": "f32", "value": 1.5 }]
                    }
                },
                "diagnostics": []
            })
        );
    }

    #[test]
    fn helper_fallbacks_and_read_responses_are_covered() {
        let node = value_node("value_1", FLOAT_KIND, json!(0.5));
        let address = RuntimeControlReadRequest {
            node_id: "value_1".to_owned(),
            target: RuntimeControlReadTarget::Param,
            id: "value".to_owned(),
        };

        assert!(supports_runtime_control_events(UI_BUTTON_KIND));
        assert!(!supports_runtime_control_events("core.target"));
        assert_eq!(
            ControlState::from_graph(&graph(vec![node.clone()]))
                .output_value_for_node(&node, "value",),
            Some(ControlValue::float(0.5))
        );
        assert_eq!(
            ControlState::from_graph(&graph(vec![node.clone()]))
                .output_value_for_node(&node, "other",),
            None
        );

        assert_eq!(
            ControlMessage::from_value(ControlValue::float(1.25)).to_text(),
            "float 1.25"
        );
        assert_eq!(
            ControlMessage::from_value(ControlValue::int(7)).to_text(),
            "int 7"
        );
        assert_eq!(
            ControlMessage::from_value(ControlValue::bool(true)).to_text(),
            "bool on"
        );
        assert_eq!(
            ControlMessage::from_value(ControlValue::bool(false)).to_text(),
            "bool off"
        );
        assert_eq!(
            ControlMessage::from_value(ControlValue::color([1.0, 0.5, 0.0, 1.0])).to_text(),
            "color color 1 0.5 0 1"
        );
        let selector_only = ControlMessage {
            selector: "clear".to_owned(),
            atoms: Vec::new(),
        };
        assert_eq!(data_kind_for_control_message(&selector_only), "message.any");
        assert_eq!(set_message_text(&selector_only), "clear");
        assert_eq!(
            set_message_text(&ControlMessage {
                selector: "set".to_owned(),
                atoms: vec![
                    ControlValue::float(1.5),
                    ControlValue::int(2),
                    ControlValue::uint(3),
                    ControlValue::bool(true),
                    ControlValue::bool(false),
                    ControlValue::string("label".to_owned()),
                    ControlValue::color([1.0, 0.5, 0.0, 1.0])
                ]
            }),
            "1.5 2 3 on off label color 1 0.5 0 1"
        );
        assert_eq!(silent_set_message(&ControlMessage::bang()), None);
        assert_eq!(
            value_from_message(
                &ControlMessage::from_value(ControlValue::int(3)),
                &ControlValue::float(0.0)
            ),
            Some(ControlValue::float(3.0))
        );
        assert_eq!(
            value_from_message(
                &ControlMessage::from_value(ControlValue::float(3.0)),
                &ControlValue::int(0)
            ),
            Some(ControlValue::int(3))
        );
        assert_eq!(
            value_from_message(
                &ControlMessage::from_value(ControlValue::float(3.0)),
                &ControlValue::uint(0)
            ),
            Some(ControlValue::uint(3))
        );
        assert_eq!(
            value_from_message(
                &ControlMessage::parse_text("on"),
                &ControlValue::bool(false)
            ),
            Some(ControlValue::bool(true))
        );
        assert_eq!(
            value_from_message(
                &ControlMessage::from_value(ControlValue::string("direct".to_owned())),
                &ControlValue::string(String::new())
            ),
            Some(ControlValue::string("direct".to_owned()))
        );
        assert_eq!(
            value_from_message(
                &ControlMessage::parse_text("route 1"),
                &ControlValue::string(String::new())
            ),
            Some(ControlValue::string("route 1".to_owned()))
        );
        assert_eq!(
            value_from_message(
                &ControlMessage::from_value(ControlValue::color([0.1, 0.2, 0.3, 1.0])),
                &ControlValue::color([1.0, 1.0, 1.0, 1.0])
            ),
            Some(ControlValue::color([0.1, 0.2, 0.3, 1.0]))
        );
        assert_eq!(
            value_from_message(
                &ControlMessage::from_value(ControlValue::float(0.5)),
                &ControlValue::color([1.0, 1.0, 1.0, 1.0])
            ),
            None
        );
        assert_eq!(
            message_from_message_node_state(&ControlValue::float(2.0)),
            ControlMessage::from_value(ControlValue::float(2.0))
        );
        assert_eq!(
            coerce_toggle_input(&ControlMessage::parse_text("0"), true),
            Some(false)
        );
        assert_eq!(
            coerce_toggle_input(&ControlMessage::parse_text("1"), false),
            Some(true)
        );
        assert_eq!(
            coerce_toggle_input(&ControlMessage::parse_text("2"), false),
            None
        );
        assert_eq!(
            coerce_toggle_input(&ControlMessage::parse_text("bang"), false),
            Some(true)
        );
        assert_eq!(
            coerce_toggle_input(&ControlMessage::parse_text("maybe"), false),
            None
        );
        assert_eq!(
            coerce_toggle_input(
                &ControlMessage {
                    selector: "0".to_owned(),
                    atoms: Vec::new()
                },
                true
            ),
            Some(false)
        );
        assert_eq!(
            coerce_toggle_input(
                &ControlMessage {
                    selector: "1".to_owned(),
                    atoms: Vec::new()
                },
                false
            ),
            Some(true)
        );
        assert_eq!(
            coerce_toggle_input(
                &ControlMessage {
                    selector: "pulse".to_owned(),
                    atoms: Vec::new()
                },
                false
            ),
            None
        );
        assert_eq!(
            coerce_toggle_input(&ControlMessage::from_value(ControlValue::uint(0)), true),
            Some(false)
        );
        assert_eq!(
            coerce_toggle_input(&ControlMessage::from_value(ControlValue::uint(1)), false),
            Some(true)
        );
        assert_eq!(
            coerce_toggle_input(&ControlMessage::from_value(ControlValue::uint(2)), false),
            None
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

        let response = state.apply_ui_event(&node, bang_request("custom_ui", "value"));

        assert!(!response.ok);
        assert!(
            response.diagnostics[0]
                .message
                .contains("does not support runtime control events")
        );
    }

    #[test]
    fn invalid_events_do_not_mutate_state() {
        let graph = graph(vec![value_node("value_1", FLOAT_KIND, json!(1.0))]);
        let mut state = ControlState::from_graph(&graph);

        for request in [
            value_request("missing", "set", ControlValue::float(2.0)),
            value_request("value_1", "value", ControlValue::float(2.0)),
            value_request("value_1", "set", ControlValue::bool(true)),
            value_request("value_1", "bang", ControlValue::float(2.0)),
        ] {
            let response = state.apply_event(request, &graph);
            assert!(!response.ok);
            assert!(response.emitted.is_empty());
            assert!(!response.diagnostics.is_empty());
            assert_eq!(
                state.value_for_node("value_1"),
                Some(&ControlValue::float(1.0))
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
            ControlValue::string("not-bool".to_owned()),
        );

        let corrupt = state.apply_event(bang_request("toggle_1", "bang"), &graph);
        assert!(!corrupt.ok);
        assert!(
            corrupt.diagnostics[0]
                .message
                .contains("non-boolean toggle state")
        );

        state
            .values
            .insert("toggle_1".to_owned(), ControlValue::bool(false));
        let unsupported = state.apply_event(bang_request("toggle_1", "other"), &graph);
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
            value_node("value_1", FLOAT_KIND, json!(1.0)),
            value_node("target_1", "core.target", json!(1.0)),
        ]);

        let mut state = ControlState::from_graph(&graph);
        let non_control = state.apply_event(
            value_request("target_1", "set", ControlValue::float(2.0)),
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
            value_request("value_1", "set", ControlValue::float(2.0)),
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

    fn edge(from_node: &str, from_port: &str, to_node: &str, to_port: &str) -> crate::Edge {
        crate::Edge {
            from: crate::PortRef {
                node: from_node.to_owned(),
                port: from_port.to_owned(),
            },
            to: crate::PortRef {
                node: to_node.to_owned(),
                port: to_port.to_owned(),
            },
        }
    }

    fn value_node(id: &str, kind: &str, value: serde_json::Value) -> GraphNode {
        let mut params = Map::new();
        params.insert("value".to_owned(), value);
        let ports = match kind {
            FLOAT_KIND => stored_value_ports("number.float"),
            INT_KIND => stored_value_ports("number.int"),
            BOOL_KIND | TOGGLE_KIND => stored_value_ports("boolean"),
            COLOR_KIND => stored_value_ports("color"),
            STRING_KIND => stored_value_ports("string"),
            MESSAGE_KIND => message_ports(),
            UI_SLIDER_FLOAT_KIND => stored_value_ports("number.float"),
            UI_TOGGLE_KIND => ui_toggle_ports(),
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

    fn ui_button_node(id: &str) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            kind: UI_BUTTON_KIND.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params: Map::new(),
            ports: vec![
                port(
                    "in",
                    PortDirection::Input,
                    DataFlow::Value,
                    "message.any",
                    Some(PortActivation::Trigger),
                ),
                port(
                    "bang",
                    PortDirection::Output,
                    DataFlow::Event,
                    "event.bang",
                    None,
                ),
            ],
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
                "in",
                PortDirection::Input,
                DataFlow::Value,
                "message.any",
                Some(PortActivation::Trigger),
            ),
            port(
                "set",
                PortDirection::Input,
                DataFlow::Value,
                "message.any",
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
                "string",
                None,
            ),
        ]
    }

    fn ui_toggle_ports() -> Vec<Port> {
        vec![
            port(
                "in",
                PortDirection::Input,
                DataFlow::Value,
                "message.any",
                Some(PortActivation::Trigger),
            ),
            port(
                "set",
                PortDirection::Input,
                DataFlow::Value,
                "message.any",
                Some(PortActivation::Latched),
            ),
            port(
                "value",
                PortDirection::Output,
                DataFlow::Value,
                "boolean",
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
