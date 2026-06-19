use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ControlMessage, ControlValue, GraphDocument, GraphNode, PortDirection, RuntimeDiagnostic,
    control_value::{
        BANG_KIND, BOOL_KIND, COLOR_KIND, FLOAT_KIND, INT_KIND, MESSAGE_KIND, PANEL_KIND,
        STRING_KIND, UINT_KIND,
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
        let mut staged = self.clone();
        let response = staged.apply_event_direct(request, graph);
        if !response.ok {
            return response;
        }

        let response = staged.propagate_emissions(response, graph);
        if response.ok {
            *self = staged;
        }
        response
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

        if node.kind == BANG_KIND {
            if request.port_id != "in" {
                return unsupported_runtime_control_port(node, &request.port_id);
            }
            return RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                node_id: node.id.clone(),
                port_id: "out".to_owned(),
                message: ControlMessage::bang(),
            }]);
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
                if node.kind == PANEL_KIND {
                    let next = set_message_text(&message);
                    self.values
                        .insert(node.id.clone(), ControlValue::string(next));
                    return RuntimeControlEventResponse::ok(Vec::new());
                }
                unsupported_runtime_control_port(node, &request.port_id)
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
                        port_id: "out".to_owned(),
                        message: message_from_message_node_state(&stored),
                    }]);
                }
                if is_toggle_widget(node) {
                    return self.apply_toggle_event(node, false, message, stored);
                }
                if is_bang_message(&message) {
                    return RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                        node_id: node.id.clone(),
                        port_id: "value".to_owned(),
                        message: ControlMessage::from_value(stored),
                    }]);
                }
                let silent = message.selector == "set";
                let Some(next) = value_from_message(&message, &stored) else {
                    return RuntimeControlEventResponse::error(type_error_from_message(
                        &message, &stored, &node.id,
                    ));
                };
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
            "cold" => {
                if node.kind == MESSAGE_KIND {
                    return unsupported_runtime_control_port(node, &request.port_id);
                }
                if is_bang_message(&message) {
                    return RuntimeControlEventResponse::error(format!(
                        "control input {}.cold does not accept bang",
                        node.id
                    ));
                }
                if is_toggle_widget(node) {
                    return self.apply_toggle_event(node, true, message, stored);
                }
                let Some(next) = value_from_message(&message, &stored) else {
                    return RuntimeControlEventResponse::error(type_error_from_message(
                        &message, &stored, &node.id,
                    ));
                };
                self.values.insert(node.id.clone(), next);
                RuntimeControlEventResponse::ok(Vec::new())
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
            let channel_response = self.publish_object_channel(&emission, graph);
            response.diagnostics.extend(channel_response.diagnostics);
            for channel_emission in channel_response.emitted {
                queue.push(channel_emission.clone());
                response.emitted.push(channel_emission);
            }
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

    fn publish_object_channel(
        &mut self,
        emission: &RuntimeControlEmission,
        graph: &GraphDocument,
    ) -> RuntimeControlEventResponse {
        let Some(source_node) = graph.nodes.iter().find(|node| node.id == emission.node_id) else {
            return RuntimeControlEventResponse::ok(Vec::new());
        };
        let data_kind = data_kind_for_control_message(&emission.message);
        let Some(channel_name) = read_named_param(source_node, "sendName") else {
            return RuntimeControlEventResponse::ok(Vec::new());
        };
        let key = format!("{data_kind}:{channel_name}");
        self.channels.insert(key, emission.message.clone());
        self.apply_receive_name_updates(
            data_kind,
            &channel_name,
            &emission.message,
            &emission.node_id,
            graph,
        )
    }

    fn apply_receive_name_updates(
        &mut self,
        data_kind: &'static str,
        channel_name: &str,
        message: &ControlMessage,
        source_node_id: &str,
        graph: &GraphDocument,
    ) -> RuntimeControlEventResponse {
        let mut emitted = Vec::new();
        let mut diagnostics = Vec::new();
        for node in graph.nodes.iter().filter(|node| node.id != source_node_id) {
            if read_named_param(node, "receiveName").as_deref() != Some(channel_name) {
                continue;
            }
            if !object_accepts_data_kind(node, data_kind) {
                diagnostics.push(RuntimeDiagnostic::warning(format!(
                    "receiveName {channel_name} on node {} ignored incompatible routed {data_kind}",
                    node.id
                )));
                continue;
            }
            let target_port = if node.kind == PANEL_KIND { "set" } else { "in" };
            let response = self.apply_event_direct(
                RuntimeControlEventRequest {
                    node_id: node.id.clone(),
                    port_id: target_port.to_owned(),
                    message: message.clone(),
                },
                graph,
            );
            if response.ok {
                emitted.extend(response.emitted);
                diagnostics.extend(response.diagnostics);
            } else {
                let detail = response
                    .diagnostics
                    .first()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .unwrap_or("unknown receiver error");
                diagnostics.push(RuntimeDiagnostic::warning(format!(
                    "receiveName {channel_name} on node {} rejected routed {data_kind}: {detail}",
                    node.id
                )));
            }
        }
        RuntimeControlEventResponse::ok_with_diagnostics(emitted, diagnostics)
    }

    fn apply_toggle_event(
        &mut self,
        node: &GraphNode,
        silent: bool,
        message: ControlMessage,
        stored: ControlValue,
    ) -> RuntimeControlEventResponse {
        let ControlValue::Bool { value: current } = stored else {
            return RuntimeControlEventResponse::error(format!(
                "node {} has non-boolean toggle state",
                node.id
            ));
        };
        let silent = silent || message.selector == "set";
        let Some(next_bool) = coerce_toggle_input(&message, current) else {
            return RuntimeControlEventResponse::error(format!(
                "control input {} expects bang, bool, 0/1, or on/off",
                node.id
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

    fn ok_with_diagnostics(
        emitted: Vec<RuntimeControlEmission>,
        diagnostics: Vec<RuntimeDiagnostic>,
    ) -> Self {
        Self {
            ok: true,
            changed: false,
            control_revision: None,
            emitted,
            diagnostics,
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
            | MESSAGE_KIND
            | PANEL_KIND
    )
}

pub fn supports_runtime_control_events(kind: &str) -> bool {
    is_control_value_kind(kind) || kind == BANG_KIND
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
        FLOAT_KIND | INT_KIND | UINT_KIND => is_numeric_data_kind(data_kind),
        BOOL_KIND => data_kind == "boolean",
        COLOR_KIND => data_kind == "color",
        STRING_KIND | PANEL_KIND => data_kind == "string",
        MESSAGE_KIND | BANG_KIND => is_control_message_data_kind(data_kind),
        _ => false,
    }
}

fn is_control_message_data_kind(data_kind: &'static str) -> bool {
    matches!(
        data_kind,
        "number.float"
            | "number.int"
            | "number.uint"
            | "boolean"
            | "color"
            | "string"
            | "event.bang"
            | "message.any"
    )
}

fn is_numeric_data_kind(data_kind: &'static str) -> bool {
    matches!(data_kind, "number.float" | "number.int" | "number.uint")
}

fn is_toggle_widget(node: &GraphNode) -> bool {
    node.kind == BOOL_KIND
        && matches!(
            node.params.get("widget").and_then(Value::as_str),
            Some("toggle" | "checkbox")
        )
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
            if message.selector == "set" {
                return Some(ControlValue::string(set_message_text(message)));
            }
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
    if message.selector == "symbol"
        && let Some(ControlValue::String { value }) = message.first_atom()
    {
        return value.clone();
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

    fn set_value_request(
        node_id: &str,
        port_id: &str,
        value: ControlValue,
    ) -> RuntimeControlEventRequest {
        request(
            node_id,
            port_id,
            ControlMessage {
                selector: "set".to_owned(),
                atoms: vec![value],
            },
        )
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
            value_node("toggle", BOOL_KIND, json!(false)),
            value_node("message", MESSAGE_KIND, json!("perform")),
            value_node("slider", FLOAT_KIND, json!(0.75)),
            value_node("ui_toggle", BOOL_KIND, json!(true)),
            value_node("other", "debug.sink", json!(10)),
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
            set_value_request("value_1", "in", ControlValue::float(32.0)),
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
    fn cold_updates_without_emission() {
        let graph = graph(vec![value_node("value_1", FLOAT_KIND, json!(1.0))]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            value_request("value_1", "cold", ControlValue::float(32.0)),
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

        let response = state.apply_event(bang_request("value_1", "in"), &graph);

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
        let graph = graph(vec![value_node("toggle_1", BOOL_KIND, json!(false))]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(bang_request("toggle_1", "in"), &graph);

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
            value_node("toggle_1", BOOL_KIND, json!(false)),
            value_node("core_toggle_1", BOOL_KIND, json!(false)),
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
            request("toggle_1", "in", ControlMessage::parse_text("set off")),
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

        let cold_toggle = state.apply_event(
            request("core_toggle_1", "cold", ControlMessage::parse_text("off")),
            &graph,
        );
        assert!(cold_toggle.ok);
        assert!(cold_toggle.emitted.is_empty());
        assert_eq!(
            state.value_for_node("core_toggle_1"),
            Some(&ControlValue::bool(false))
        );

        let float_zero = state.apply_event(
            value_request("toggle_1", "in", ControlValue::float(0.0)),
            &graph,
        );
        assert!(float_zero.ok);
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::bool(false))
        );

        let float_one = state.apply_event(
            value_request("toggle_1", "in", ControlValue::float(1.0)),
            &graph,
        );
        assert!(float_one.ok);
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::bool(true))
        );

        let float_mid_rejected = state.apply_event(
            value_request("toggle_1", "in", ControlValue::float(0.5)),
            &graph,
        );
        assert!(!float_mid_rejected.ok);
        assert!(
            float_mid_rejected.diagnostics[0]
                .message
                .contains("expects bang, bool, 0/1, or on/off")
        );

        let color_rejected = state.apply_event(
            value_request("toggle_1", "in", ControlValue::color([0.0, 0.0, 0.0, 1.0])),
            &graph,
        );
        assert!(!color_rejected.ok);
        assert!(
            color_rejected.diagnostics[0]
                .message
                .contains("expects bang, bool, 0/1, or on/off")
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

        let message_response = state.apply_event(bang_request("message_1", "in"), &graph);
        assert!(message_response.ok);
        assert_eq!(
            message_response.emitted,
            vec![RuntimeControlEmission {
                node_id: "message_1".to_owned(),
                port_id: "out".to_owned(),
                message: ControlMessage::parse_text("perform")
            }]
        );

        let set_response = state.apply_event(
            request("message_1", "in", ControlMessage::parse_text("set updated")),
            &graph,
        );
        assert!(set_response.ok);
        assert!(set_response.emitted.is_empty());
        assert_eq!(
            state.value_for_node("message_1"),
            Some(&ControlValue::string("updated".to_owned()))
        );

        let color_set = state.apply_event(
            request(
                "message_1",
                "in",
                ControlMessage::parse_text("set color 1 0.5 0.25 1"),
            ),
            &graph,
        );
        assert!(color_set.ok);
        assert_eq!(
            state.value_for_node("message_1"),
            Some(&ControlValue::string("color 1 0.5 0.25 1".to_owned()))
        );
        let color_emit = state.apply_event(bang_request("message_1", "in"), &graph);
        assert!(color_emit.ok);
        assert_eq!(
            color_emit.emitted[0].message,
            ControlMessage::from_value(ControlValue::color([1.0, 0.5, 0.25, 1.0]))
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

        let cold_message = state.apply_event(
            request(
                "message_1",
                "cold",
                ControlMessage::parse_text("set ignored"),
            ),
            &graph,
        );
        assert!(!cold_message.ok);
        assert!(
            cold_message.diagnostics[0]
                .message
                .contains("does not support runtime control input port cold")
        );

        let string_set = state.apply_event(
            request("string_1", "in", ControlMessage::parse_text("set armed")),
            &graph,
        );
        assert!(string_set.ok);
        assert!(string_set.emitted.is_empty());
        assert_eq!(
            state.value_for_node("string_1"),
            Some(&ControlValue::string("armed".to_owned()))
        );

        let emit_in = state.apply_event(bang_request("message_1", "in"), &graph);
        assert!(emit_in.ok);
        assert_eq!(
            emit_in.emitted,
            vec![RuntimeControlEmission {
                node_id: "message_1".to_owned(),
                port_id: "out".to_owned(),
                message: ControlMessage::parse_text("queued")
            }]
        );
    }

    #[test]
    fn object_send_name_dispatches_to_receive_name_inlet() {
        let mut sender = value_node("slider_1", FLOAT_KIND, json!(0.25));
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
            response.emitted,
            vec![
                RuntimeControlEmission {
                    node_id: "slider_1".to_owned(),
                    port_id: "value".to_owned(),
                    message: ControlMessage::from_value(ControlValue::float(1.25)),
                },
                RuntimeControlEmission {
                    node_id: "value_1".to_owned(),
                    port_id: "value".to_owned(),
                    message: ControlMessage::from_value(ControlValue::float(1.25)),
                },
            ]
        );
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::float(1.25))
        );

        let mut bang_sender = bang_node("button_1");
        bang_sender
            .params
            .insert("sendName".to_owned(), json!("go"));
        let graph = graph(vec![bang_sender]);
        let mut state = ControlState::from_graph(&graph);
        let bang = state.apply_event(bang_request("button_1", "in"), &graph);
        assert!(bang.ok);
        assert_eq!(
            state.channels.get("event.bang:go"),
            Some(&ControlMessage::bang())
        );
    }

    #[test]
    fn object_receive_name_dispatches_any_message_to_bang() {
        let mut sender = value_node("message_1", MESSAGE_KIND, json!("perform"));
        sender.params.insert("sendName".to_owned(), json!("go"));
        let mut receiver = bang_node("bang_1");
        receiver
            .params
            .insert("receiveName".to_owned(), json!("go"));
        let routing_graph = graph(vec![sender, receiver]);
        let mut state = ControlState::from_graph(&routing_graph);

        let response = state.apply_event(bang_request("message_1", "in"), &routing_graph);

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![
                RuntimeControlEmission {
                    node_id: "message_1".to_owned(),
                    port_id: "out".to_owned(),
                    message: ControlMessage::parse_text("perform"),
                },
                RuntimeControlEmission {
                    node_id: "bang_1".to_owned(),
                    port_id: "out".to_owned(),
                    message: ControlMessage::bang(),
                },
            ]
        );
    }

    #[test]
    fn object_receive_name_dispatches_float_to_bang() {
        let mut sender = value_node("slider_1", FLOAT_KIND, json!(0.25));
        sender.params.insert("sendName".to_owned(), json!("go"));
        let mut receiver = bang_node("bang_1");
        receiver
            .params
            .insert("receiveName".to_owned(), json!("go"));
        let routing_graph = graph(vec![sender, receiver]);
        let mut state = ControlState::from_graph(&routing_graph);

        let response = state.apply_event(
            value_request("slider_1", "in", ControlValue::float(1.25)),
            &routing_graph,
        );

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![
                RuntimeControlEmission {
                    node_id: "slider_1".to_owned(),
                    port_id: "value".to_owned(),
                    message: ControlMessage::from_value(ControlValue::float(1.25)),
                },
                RuntimeControlEmission {
                    node_id: "bang_1".to_owned(),
                    port_id: "out".to_owned(),
                    message: ControlMessage::bang(),
                },
            ]
        );
    }

    #[test]
    fn receive_name_dispatch_uses_numeric_conversion_policy() {
        let mut sender = value_node("float_sender", FLOAT_KIND, json!(0.25));
        sender.params.insert("sendName".to_owned(), json!("number"));
        let mut int_receiver = value_node("int_receiver", INT_KIND, json!(0));
        int_receiver
            .params
            .insert("receiveName".to_owned(), json!("number"));
        let mut uint_receiver = value_node("uint_receiver", UINT_KIND, json!(0));
        uint_receiver
            .params
            .insert("receiveName".to_owned(), json!("number"));
        let routing_graph = graph(vec![sender, int_receiver, uint_receiver]);
        let mut state = ControlState::from_graph(&routing_graph);

        let response = state.apply_event(
            value_request("float_sender", "in", ControlValue::float(12.9)),
            &routing_graph,
        );

        assert!(response.ok);
        assert!(
            response.diagnostics.is_empty(),
            "{:?}",
            response.diagnostics
        );
        assert_eq!(
            state.value_for_node("int_receiver"),
            Some(&ControlValue::int(12))
        );
        assert_eq!(
            state.value_for_node("uint_receiver"),
            Some(&ControlValue::uint(12))
        );
    }

    #[test]
    fn panel_set_port_updates_runtime_color_text_silently() {
        let graph = graph(vec![panel_node("panel_1")]);
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            request("panel_1", "set", ControlMessage::parse_text("set #00ff00")),
            &graph,
        );

        assert!(response.ok);
        assert!(response.emitted.is_empty());
        assert_eq!(
            state.value_for_node("panel_1"),
            Some(&ControlValue::string("#00ff00".to_owned()))
        );
    }

    #[test]
    fn object_receive_name_dispatches_string_channels_to_panel_set_port() {
        let mut sender = value_node("string_1", STRING_KIND, json!("idle"));
        sender.params.insert("sendName".to_owned(), json!("status"));
        let mut receiver = panel_node("panel_1");
        receiver
            .params
            .insert("receiveName".to_owned(), json!("status"));
        let routing_graph = graph(vec![sender, receiver]);
        let mut state = ControlState::from_graph(&routing_graph);

        let response = state.apply_event(
            request(
                "string_1",
                "in",
                ControlMessage::from_value(ControlValue::string("ready".to_owned())),
            ),
            &routing_graph,
        );

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![RuntimeControlEmission {
                node_id: "string_1".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::string("ready".to_owned())),
            }]
        );
        assert_eq!(
            state.value_for_node("panel_1"),
            Some(&ControlValue::string("ready".to_owned()))
        );
    }

    #[test]
    fn object_channel_helpers_skip_missing_sources_empty_names_and_mismatched_receivers() {
        let mut sender = value_node("slider_1", FLOAT_KIND, json!(0.25));
        sender.params.insert("sendName".to_owned(), json!("   "));
        let mut wrong_receiver = value_node("bool_1", BOOL_KIND, json!(false));
        wrong_receiver
            .params
            .insert("receiveName".to_owned(), json!("speed"));
        let empty_name_graph = graph(vec![sender, wrong_receiver]);
        let mut state = ControlState::from_graph(&empty_name_graph);

        let missing_source = state.publish_object_channel(
            &RuntimeControlEmission {
                node_id: "missing".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::float(1.0)),
            },
            &empty_name_graph,
        );
        assert!(missing_source.ok);
        let empty_name = state.publish_object_channel(
            &RuntimeControlEmission {
                node_id: "slider_1".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::float(1.0)),
            },
            &empty_name_graph,
        );
        assert!(empty_name.ok);
        assert!(state.channels.is_empty());
        assert_eq!(
            state.value_for_node("bool_1"),
            Some(&ControlValue::bool(false))
        );

        let mut sender = value_node("slider_2", FLOAT_KIND, json!(0.25));
        sender.params.insert("sendName".to_owned(), json!("speed"));
        let mut wrong_receiver = value_node("bool_2", BOOL_KIND, json!(false));
        wrong_receiver
            .params
            .insert("receiveName".to_owned(), json!("speed"));
        let mismatched_receiver_graph = graph(vec![sender, wrong_receiver]);
        let mut mismatched_state = ControlState::from_graph(&mismatched_receiver_graph);
        let mismatched = mismatched_state.publish_object_channel(
            &RuntimeControlEmission {
                node_id: "slider_2".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::float(1.0)),
            },
            &mismatched_receiver_graph,
        );
        assert!(mismatched.ok);
        assert_eq!(mismatched.diagnostics.len(), 1);
        assert!(
            mismatched.diagnostics[0]
                .message
                .contains("ignored incompatible routed number.float")
        );
        assert_eq!(
            mismatched_state.value_for_node("bool_2"),
            Some(&ControlValue::bool(false))
        );

        let mut sender = value_node("string_sender", STRING_KIND, json!("ready"));
        sender.params.insert("sendName".to_owned(), json!("label"));
        let mut broken_receiver = value_node("string_receiver", STRING_KIND, json!("old"));
        broken_receiver
            .params
            .insert("receiveName".to_owned(), json!("label"));
        let rejected_receiver_graph = graph(vec![sender, broken_receiver]);
        let mut rejected_state = ControlState::from_graph(&rejected_receiver_graph);
        rejected_state
            .values
            .insert("string_receiver".to_owned(), ControlValue::float(0.0));
        let rejected = rejected_state.publish_object_channel(
            &RuntimeControlEmission {
                node_id: "string_sender".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::string("new".to_owned())),
            },
            &rejected_receiver_graph,
        );
        assert!(rejected.ok);
        assert_eq!(rejected.diagnostics.len(), 1);
        assert!(
            rejected.diagnostics[0]
                .message
                .contains("rejected routed string")
        );
        assert_eq!(
            rejected_state.value_for_node("string_receiver"),
            Some(&ControlValue::float(0.0))
        );

        let mut sender = value_node("slider_3", FLOAT_KIND, json!(0.25));
        sender.params.insert("sendName".to_owned(), json!("speed"));
        let mut other_receiver = value_node("value_3", FLOAT_KIND, json!(0.0));
        other_receiver
            .params
            .insert("receiveName".to_owned(), json!("other"));
        let different_name_graph = graph(vec![sender, other_receiver]);
        let mut different_name_state = ControlState::from_graph(&different_name_graph);
        different_name_state.publish_object_channel(
            &RuntimeControlEmission {
                node_id: "slider_3".to_owned(),
                port_id: "value".to_owned(),
                message: ControlMessage::from_value(ControlValue::float(1.0)),
            },
            &different_name_graph,
        );
        assert_eq!(
            different_name_state.value_for_node("value_3"),
            Some(&ControlValue::float(0.0))
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
            &value_node("message_1", MESSAGE_KIND, json!("go")),
            "number.float"
        ));
        assert!(object_accepts_data_kind(
            &bang_node("button_1"),
            "event.bang"
        ));
        assert!(object_accepts_data_kind(
            &bang_node("button_1"),
            "number.float"
        ));
        assert!(object_accepts_data_kind(&bang_node("button_1"), "string"));
        for data_kind in [
            "number.float",
            "number.int",
            "number.uint",
            "boolean",
            "color",
            "string",
            "event.bang",
            "message.any",
        ] {
            assert!(
                object_accepts_data_kind(&bang_node("button_1"), data_kind),
                "bang should accept {data_kind}"
            );
            assert!(
                object_accepts_data_kind(
                    &value_node("message_1", MESSAGE_KIND, json!("go")),
                    data_kind
                ),
                "message should accept {data_kind}"
            );
        }
        assert!(!object_accepts_data_kind(
            &value_node("target_1", "debug.sink", json!(null)),
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
            set_value_request("value_1", "in", ControlValue::float(2.0)),
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
            value_node("slider_1", FLOAT_KIND, json!(0.25)),
            value_node("value_1", FLOAT_KIND, json!(0.0)),
            value_node("message_1", MESSAGE_KIND, json!("go")),
            bang_node("button_1"),
        ]);
        graph.edges = vec![
            edge("slider_1", "value", "value_1", "in"),
            edge("button_1", "out", "message_1", "in"),
        ];
        let mut state = ControlState::from_graph(&graph);

        let slider = state.apply_event(
            value_request("slider_1", "in", ControlValue::float(1.5)),
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
                    port_id: "out".to_owned(),
                    message: ControlMessage::bang()
                },
                RuntimeControlEmission {
                    node_id: "message_1".to_owned(),
                    port_id: "out".to_owned(),
                    message: ControlMessage::parse_text("go")
                }
            ]
        );
    }

    #[test]
    fn bang_to_float_to_float_propagates_stored_value() {
        let mut graph = graph(vec![
            bang_node("button_1"),
            value_node("float_a", FLOAT_KIND, json!(7.25)),
            value_node("float_b", FLOAT_KIND, json!(0.0)),
        ]);
        graph.edges = vec![
            edge("button_1", "out", "float_a", "in"),
            edge("float_a", "value", "float_b", "in"),
        ];
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(bang_request("button_1", "in"), &graph);

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![
                RuntimeControlEmission {
                    node_id: "button_1".to_owned(),
                    port_id: "out".to_owned(),
                    message: ControlMessage::bang(),
                },
                RuntimeControlEmission {
                    node_id: "float_a".to_owned(),
                    port_id: "value".to_owned(),
                    message: ControlMessage::from_value(ControlValue::float(7.25)),
                },
                RuntimeControlEmission {
                    node_id: "float_b".to_owned(),
                    port_id: "value".to_owned(),
                    message: ControlMessage::from_value(ControlValue::float(7.25)),
                },
            ]
        );
        assert_eq!(
            state.value_for_node("float_b"),
            Some(&ControlValue::float(7.25))
        );
    }

    #[test]
    fn bang_to_message_to_bang_propagates_as_bang() {
        let mut graph = graph(vec![
            bang_node("button_1"),
            value_node("message_1", MESSAGE_KIND, json!("go")),
            bang_node("button_2"),
        ]);
        graph.edges = vec![
            edge("button_1", "out", "message_1", "in"),
            edge("message_1", "out", "button_2", "in"),
        ];
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(bang_request("button_1", "in"), &graph);

        assert!(response.ok);
        assert_eq!(
            response.emitted,
            vec![
                RuntimeControlEmission {
                    node_id: "button_1".to_owned(),
                    port_id: "out".to_owned(),
                    message: ControlMessage::bang(),
                },
                RuntimeControlEmission {
                    node_id: "message_1".to_owned(),
                    port_id: "out".to_owned(),
                    message: ControlMessage::parse_text("go"),
                },
                RuntimeControlEmission {
                    node_id: "button_2".to_owned(),
                    port_id: "out".to_owned(),
                    message: ControlMessage::bang(),
                },
            ]
        );
    }

    #[test]
    fn object_edge_propagation_ignores_edges_to_missing_targets() {
        let mut graph = graph(vec![
            value_node("slider_1", FLOAT_KIND, json!(0.25)),
            value_node("sink_1", "debug.sink", json!(null)),
        ]);
        graph.edges = vec![
            edge("slider_1", "value", "missing", "in"),
            edge("slider_1", "value", "sink_1", "in"),
        ];
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            value_request("slider_1", "in", ControlValue::float(1.5)),
            &graph,
        );

        assert!(response.ok);
        assert_eq!(response.emitted.len(), 1);
        assert!(state.channels.is_empty());
    }

    #[test]
    fn object_edge_propagation_rejects_invalid_target_port() {
        let mut graph = graph(vec![
            value_node("slider_1", FLOAT_KIND, json!(0.25)),
            value_node("value_1", FLOAT_KIND, json!(0.0)),
        ]);
        graph.edges = vec![edge("slider_1", "value", "value_1", "missing")];
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            value_request("slider_1", "in", ControlValue::float(1.5)),
            &graph,
        );

        assert!(!response.ok);
        assert!(response.diagnostics[0].message.contains("port missing"));
        assert!(state.channels.is_empty());
        assert_eq!(
            state.value_for_node("slider_1"),
            Some(&ControlValue::float(0.25))
        );
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::float(0.0))
        );
    }

    #[test]
    fn ui_panel_propagation_stops_at_runtime_safety_limit() {
        let mut graph = graph(vec![
            value_node("slider_1", FLOAT_KIND, json!(0.25)),
            value_node("value_1", FLOAT_KIND, json!(0.0)),
        ]);
        graph.edges = vec![
            edge("slider_1", "value", "value_1", "in"),
            edge("value_1", "value", "value_1", "in"),
        ];
        let mut state = ControlState::from_graph(&graph);

        let response = state.apply_event(
            value_request("slider_1", "in", ControlValue::float(1.5)),
            &graph,
        );

        assert!(!response.ok);
        assert!(
            response.diagnostics[0]
                .message
                .contains("runtime safety limit")
        );
        assert_eq!(
            state.value_for_node("slider_1"),
            Some(&ControlValue::float(0.25))
        );
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::float(0.0))
        );
    }

    #[test]
    fn ui_panel_controls_emit_runtime_values() {
        let graph = graph(vec![
            value_node("slider_1", FLOAT_KIND, json!(0.5)),
            value_node("toggle_1", BOOL_KIND, json!(false)),
            bang_node("button_1"),
        ]);
        let mut state = ControlState::from_graph(&graph);

        let slider = state.apply_event(
            value_request("slider_1", "in", ControlValue::float(1.25)),
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

        let toggle = state.apply_event(bang_request("toggle_1", "in"), &graph);
        assert!(toggle.ok);
        assert_eq!(
            emitted_value(&toggle.emitted[0]),
            Some(ControlValue::bool(true))
        );
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::bool(true))
        );

        let button = state.apply_event(bang_request("button_1", "in"), &graph);
        assert!(button.ok);
        assert_eq!(button.emitted[0].message, ControlMessage::bang());
    }

    #[test]
    fn ui_panel_controls_reject_wrong_ports_and_types() {
        let graph = graph(vec![
            value_node("slider_1", FLOAT_KIND, json!(0.5)),
            value_node("toggle_1", BOOL_KIND, json!(false)),
            bang_node("button_1"),
        ]);
        let mut state = ControlState::from_graph(&graph);

        for request in [
            bang_request("button_1", "value"),
            value_request("slider_1", "cold", ControlValue::bool(true)),
            value_request("slider_1", "in", ControlValue::bool(true)),
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
            set_value_request("slider_1", "in", ControlValue::float(1.0)),
            &graph,
        );
        assert!(slider_set.ok);
        assert!(slider_set.emitted.is_empty());
        assert_eq!(
            state.value_for_node("slider_1"),
            Some(&ControlValue::float(1.0))
        );

        let slider_bang = state.apply_event(bang_request("slider_1", "in"), &graph);
        assert!(slider_bang.ok);
        assert_eq!(
            emitted_value(&slider_bang.emitted[0]),
            Some(ControlValue::float(1.0))
        );

        let slider_bad_bang = state.apply_event(
            value_request("slider_1", "in", ControlValue::bool(true)),
            &graph,
        );
        assert!(!slider_bad_bang.ok);

        let slider_other = state.apply_event(
            value_request("slider_1", "other", ControlValue::float(1.0)),
            &graph,
        );
        assert!(!slider_other.ok);

        state.values.remove("slider_1");
        let slider_missing_state = state.apply_event(bang_request("slider_1", "in"), &graph);
        assert!(!slider_missing_state.ok);
        assert!(
            slider_missing_state.diagnostics[0]
                .message
                .contains("has no runtime control state")
        );

        let bool_toggle = state.apply_event(
            value_request("toggle_1", "in", ControlValue::bool(true)),
            &graph,
        );
        assert!(bool_toggle.ok);
        assert_eq!(
            state.value_for_node("toggle_1"),
            Some(&ControlValue::bool(true))
        );

        state.values.remove("toggle_1");
        let missing_state = state.apply_event(bang_request("toggle_1", "in"), &graph);
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

        assert!(supports_runtime_control_events(BANG_KIND));
        assert!(!supports_runtime_control_events("debug.sink"));
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
            "color 1 0.5 0 1"
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
    fn unsupported_control_kind_is_reported() {
        let graph = graph(vec![value_node("custom", "debug.sink", json!(null))]);
        let mut state = ControlState::default();

        let response = state.apply_event(bang_request("custom", "in"), &graph);

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
            value_request("missing", "cold", ControlValue::float(2.0)),
            value_request("value_1", "value", ControlValue::float(2.0)),
            value_request("value_1", "cold", ControlValue::bool(true)),
            bang_request("value_1", "cold"),
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
        let mut graph = graph(vec![
            value_node("toggle_1", BOOL_KIND, json!(false)),
            value_node("value_1", FLOAT_KIND, json!(1.0)),
            value_node("message_1", MESSAGE_KIND, json!("go")),
        ]);
        graph.nodes[0].ports.push(port(
            "other",
            PortDirection::Input,
            DataFlow::Event,
            "event.bang",
            Some(PortActivation::Trigger),
        ));
        graph.nodes[1].ports.push(port(
            "set",
            PortDirection::Input,
            DataFlow::Event,
            "message.any",
            Some(PortActivation::Trigger),
        ));
        graph.nodes[2].ports.push(port(
            "cold",
            PortDirection::Input,
            DataFlow::Value,
            "message.any",
            Some(PortActivation::Latched),
        ));
        let mut state = ControlState::from_graph(&graph);
        state.values.insert(
            "toggle_1".to_owned(),
            ControlValue::string("not-bool".to_owned()),
        );

        let corrupt = state.apply_event(bang_request("toggle_1", "in"), &graph);
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

        let unsupported_set = state.apply_event(
            request("value_1", "set", ControlMessage::parse_text("set 2")),
            &graph,
        );
        assert!(!unsupported_set.ok);
        assert!(
            unsupported_set.diagnostics[0]
                .message
                .contains("does not support runtime control input port set")
        );

        let unsupported_message_cold = state.apply_event(
            request(
                "message_1",
                "cold",
                ControlMessage::parse_text("set ignored"),
            ),
            &graph,
        );
        assert!(!unsupported_message_cold.ok);
        assert!(
            unsupported_message_cold.diagnostics[0]
                .message
                .contains("does not support runtime control input port cold")
        );
    }

    #[test]
    fn rejects_non_control_nodes_and_missing_control_state() {
        let graph = graph(vec![
            value_node("value_1", FLOAT_KIND, json!(1.0)),
            value_node("target_1", "debug.sink", json!(1.0)),
        ]);

        let mut state = ControlState::from_graph(&graph);
        let non_control = state.apply_event(
            value_request("target_1", "cold", ControlValue::float(2.0)),
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
            value_request("value_1", "cold", ControlValue::float(2.0)),
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
            UINT_KIND => stored_value_ports("number.uint"),
            BOOL_KIND => stored_value_ports("boolean"),
            COLOR_KIND => stored_value_ports("color"),
            STRING_KIND => stored_value_ports("string"),
            MESSAGE_KIND => message_ports(),
            _ => Vec::new(),
        };
        if id.contains("slider") {
            params.insert("widget".to_owned(), json!("slider"));
        }
        if id.contains("toggle") {
            params.insert("widget".to_owned(), json!("toggle"));
        }
        GraphNode {
            id: id.to_owned(),
            kind: kind.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports,
        }
    }

    fn panel_node(id: &str) -> GraphNode {
        let mut params = Map::new();
        params.insert("color".to_owned(), json!(null));
        GraphNode {
            id: id.to_owned(),
            kind: PANEL_KIND.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![port(
                "set",
                PortDirection::Input,
                DataFlow::Event,
                "message.any",
                Some(PortActivation::Trigger),
            )],
        }
    }

    fn bang_node(id: &str) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            kind: BANG_KIND.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params: Map::new(),
            ports: vec![
                port(
                    "in",
                    PortDirection::Input,
                    DataFlow::Event,
                    "message.any",
                    Some(PortActivation::Trigger),
                ),
                port(
                    "out",
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
                DataFlow::Event,
                "message.any",
                Some(PortActivation::Trigger),
            ),
            port(
                "cold",
                PortDirection::Input,
                DataFlow::Value,
                data_kind,
                Some(PortActivation::Latched),
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
                DataFlow::Event,
                "message.any",
                Some(PortActivation::Trigger),
            ),
            port(
                "out",
                PortDirection::Output,
                DataFlow::Event,
                "message.any",
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
