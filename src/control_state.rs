use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ControlMessage, ControlValue, GraphDocument, GraphNode, PortDirection, RuntimeIssue,
    control_value::{
        BANG_KIND, COLOR_KIND, COMMENT_KIND, FLOAT_KIND, INT_KIND, MESSAGE_KIND, OPERATOR_ADD_KIND,
        OPERATOR_DIV_KIND, OPERATOR_MAX_KIND, OPERATOR_MIN_KIND, OPERATOR_MUL_KIND,
        OPERATOR_POW_KIND, OPERATOR_SQRT_KIND, OPERATOR_SUB_KIND, PANEL_KIND,
        value_type_id_for_float_representation, value_type_id_for_int_representation,
    },
    convert_control_value_to_data_kind, convert_control_value_to_stored,
};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    pub values: BTreeMap<String, ControlValue>,
    pub channels: BTreeMap<String, ControlMessage>,
    #[serde(default)]
    pub operator_right: BTreeMap<String, ControlValue>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
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
    pub issues: Vec<RuntimeIssue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControlStateResponse {
    pub ok: bool,
    pub control_revision: u64,
    pub values: BTreeMap<String, ControlValue>,
    pub channels: BTreeMap<String, ControlMessage>,
    pub issues: Vec<RuntimeIssue>,
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
    pub issues: Vec<RuntimeIssue>,
}

impl RuntimeControlEventRequest {
    fn control_message(&self) -> ControlMessage {
        self.message.clone()
    }
}

impl ControlState {
    pub(crate) fn from_graph(graph: &GraphDocument) -> Self {
        let values = graph
            .nodes
            .iter()
            .filter_map(|node| {
                ControlValue::for_node_default(node).map(|value| (node.id.clone(), value))
            })
            .collect();
        let operator_right = graph
            .nodes
            .iter()
            .filter_map(|node| operator_right_default(node).map(|value| (node.id.clone(), value)))
            .collect();
        Self {
            values,
            channels: BTreeMap::new(),
            operator_right,
        }
    }

    pub fn value_for_node(&self, node_id: &str) -> Option<&ControlValue> {
        self.values.get(node_id)
    }

    pub(crate) fn output_value_for_node(
        &self,
        node: &GraphNode,
        port_id: &str,
    ) -> Option<ControlValue> {
        if port_id != "value" && !(is_control_operator_kind(&node.kind) && port_id == "out") {
            return None;
        }
        self.values.get(&node.id).cloned()
    }

    pub(crate) fn apply_event(
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

        let message = request.control_message();

        if is_control_operator_kind(&node.kind) {
            return self.apply_operator_event(node, &request.port_id, message);
        }

        let Some(stored) = self.values.get(&node.id).cloned() else {
            return RuntimeControlEventResponse::error(format!(
                "node {} has no runtime control state",
                node.id
            ));
        };

        match request.port_id.as_str() {
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
                if node.kind == COMMENT_KIND || node.kind == PANEL_KIND {
                    let Some(next) = silent_set_message(&message) else {
                        return RuntimeControlEventResponse::error(format!(
                            "control input {} expects set message",
                            node.id
                        ));
                    };
                    self.values
                        .insert(node.id.clone(), ControlValue::string(next));
                    return RuntimeControlEventResponse::ok(Vec::new());
                }
                if is_bang_message(&message) {
                    return RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                        node_id: node.id.clone(),
                        port_id: "value".to_owned(),
                        message: ControlMessage::from_value(stored),
                    }]);
                }
                let silent = message.key == "set";
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
                if message.key == "set" {
                    return RuntimeControlEventResponse::error(format!(
                        "control input {}.cold expects a typed control payload",
                        node.id
                    ));
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
            response.issues.extend(channel_response.issues);
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
        let mut issues = Vec::new();
        for node in graph.nodes.iter().filter(|node| node.id != source_node_id) {
            if read_named_param(node, "receiveName").as_deref() != Some(channel_name) {
                continue;
            }
            if !object_accepts_data_kind(node, data_kind) {
                issues.push(RuntimeIssue::warning(format!(
                    "receiveName {channel_name} on node {} ignored incompatible routed {data_kind}",
                    node.id
                )));
                continue;
            }
            let response = self.apply_event_direct(
                RuntimeControlEventRequest {
                    node_id: node.id.clone(),
                    port_id: "in".to_owned(),
                    message: message.clone(),
                },
                graph,
            );
            if response.ok {
                emitted.extend(response.emitted);
                issues.extend(response.issues);
            } else {
                let detail = response
                    .issues
                    .first()
                    .map(|issue| issue.message.as_str())
                    .unwrap_or("unknown receiver error");
                issues.push(RuntimeIssue::warning(format!(
                    "receiveName {channel_name} on node {} rejected routed {data_kind}: {detail}",
                    node.id
                )));
            }
        }
        RuntimeControlEventResponse::ok_with_issues(emitted, issues)
    }

    fn apply_operator_event(
        &mut self,
        node: &GraphNode,
        port_id: &str,
        message: ControlMessage,
    ) -> RuntimeControlEventResponse {
        match port_id {
            "in" => {
                if is_bang_message(&message) {
                    let stored = self
                        .values
                        .get(&node.id)
                        .cloned()
                        .unwrap_or_else(|| ControlValue::float(0.0));
                    return RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                        node_id: node.id.clone(),
                        port_id: "out".to_owned(),
                        message: ControlMessage::from_value(stored),
                    }]);
                }

                let silent = message.key == "set";
                let Some(input) = numeric_message_value(&message) else {
                    return RuntimeControlEventResponse::error(format!(
                        "control operator {} expects a numeric message",
                        node.id
                    ));
                };
                let right = self
                    .operator_right
                    .get(&node.id)
                    .and_then(control_value_as_f64)
                    .unwrap_or_else(|| {
                        operator_right_default(node)
                            .and_then(|value| control_value_as_f64(&value))
                            .unwrap_or(0.0)
                    });
                let result =
                    operator_result_value(node, evaluate_operator(&node.kind, input, right));
                self.values.insert(node.id.clone(), result.clone());
                if silent {
                    RuntimeControlEventResponse::ok(Vec::new())
                } else {
                    RuntimeControlEventResponse::ok(vec![RuntimeControlEmission {
                        node_id: node.id.clone(),
                        port_id: "out".to_owned(),
                        message: ControlMessage::from_value(result),
                    }])
                }
            }
            "right" => {
                if is_bang_message(&message) {
                    return RuntimeControlEventResponse::error(format!(
                        "control operator {}.right does not accept bang",
                        node.id
                    ));
                }
                if message.key == "set" {
                    return RuntimeControlEventResponse::error(format!(
                        "control operator {}.right expects a typed numeric payload",
                        node.id
                    ));
                }
                let Some(right) = message
                    .first_atom()
                    .filter(|value| control_value_as_f64(value).is_some())
                    .cloned()
                else {
                    return RuntimeControlEventResponse::error(format!(
                        "control operator {}.right expects a numeric message",
                        node.id
                    ));
                };
                self.operator_right.insert(node.id.clone(), right);
                RuntimeControlEventResponse::ok(Vec::new())
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
            changed: false,
            control_revision: None,
            emitted,
            issues: Vec::new(),
        }
    }

    fn ok_with_issues(emitted: Vec<RuntimeControlEmission>, issues: Vec<RuntimeIssue>) -> Self {
        Self {
            ok: true,
            changed: false,
            control_revision: None,
            emitted,
            issues,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            changed: false,
            control_revision: None,
            emitted: Vec::new(),
            issues: vec![RuntimeIssue::error(message)],
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
        FLOAT_KIND | INT_KIND | COLOR_KIND | MESSAGE_KIND | COMMENT_KIND | PANEL_KIND
    )
}

pub fn is_control_operator_kind(kind: &str) -> bool {
    matches!(
        kind,
        OPERATOR_ADD_KIND
            | OPERATOR_SUB_KIND
            | OPERATOR_MUL_KIND
            | OPERATOR_DIV_KIND
            | OPERATOR_POW_KIND
            | OPERATOR_MIN_KIND
            | OPERATOR_MAX_KIND
            | OPERATOR_SQRT_KIND
    )
}

pub fn supports_runtime_control_events(kind: &str) -> bool {
    is_control_value_kind(kind) || is_control_operator_kind(kind) || kind == BANG_KIND
}

impl RuntimeControlReadResponse {
    pub fn ok(address: RuntimeControlReadRequest, value: Value) -> Self {
        Self {
            ok: true,
            address,
            value: Some(value),
            issues: Vec::new(),
        }
    }

    pub fn error(address: RuntimeControlReadRequest, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            address,
            value: None,
            issues: vec![RuntimeIssue::error(message)],
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
        return "value.core.bang";
    }
    match message.first_atom() {
        Some(value) => port_type_for_control_value(value),
        None => "value.core.message",
    }
}

fn port_type_for_control_value(value: &ControlValue) -> &'static str {
    match value {
        ControlValue::Float { representation, .. } => {
            value_type_id_for_float_representation(representation).unwrap_or("value.core.float32")
        }
        ControlValue::Int { representation, .. } => {
            value_type_id_for_int_representation(representation).unwrap_or("value.core.int32")
        }
        ControlValue::Uint { representation, .. } => {
            value_type_id_for_int_representation(representation).unwrap_or("value.core.uint32")
        }
        ControlValue::Bool { .. } => "value.core.bool",
        ControlValue::String { .. } => "value.core.string",
        ControlValue::Color { .. } => "value.core.color",
    }
}

fn object_accepts_data_kind(node: &GraphNode, data_kind: &'static str) -> bool {
    match node.kind.as_str() {
        FLOAT_KIND | INT_KIND => is_numeric_data_kind(data_kind),
        kind if is_control_operator_kind(kind) => is_numeric_data_kind(data_kind),
        COLOR_KIND => data_kind == "value.core.color",
        COMMENT_KIND | PANEL_KIND => {
            data_kind == "value.core.string" || data_kind == "value.core.message"
        }
        MESSAGE_KIND | BANG_KIND => is_control_message_data_kind(data_kind),
        _ => false,
    }
}

fn is_control_message_data_kind(data_kind: &'static str) -> bool {
    matches!(
        data_kind,
        "value.core.float8"
            | "value.core.float16"
            | "value.core.float32"
            | "value.core.float64"
            | "value.core.ufloat8"
            | "value.core.ufloat16"
            | "value.core.ufloat32"
            | "value.core.ufloat64"
            | "value.core.int8"
            | "value.core.int16"
            | "value.core.int32"
            | "value.core.int64"
            | "value.core.uint8"
            | "value.core.uint16"
            | "value.core.uint32"
            | "value.core.uint64"
            | "value.core.bool"
            | "value.core.color"
            | "value.core.string"
            | "value.core.bang"
            | "value.core.message"
    )
}

fn is_numeric_data_kind(data_kind: &'static str) -> bool {
    matches!(
        data_kind,
        "value.core.float8"
            | "value.core.float16"
            | "value.core.float32"
            | "value.core.float64"
            | "value.core.ufloat8"
            | "value.core.ufloat16"
            | "value.core.ufloat32"
            | "value.core.ufloat64"
            | "value.core.int8"
            | "value.core.int16"
            | "value.core.int32"
            | "value.core.int64"
            | "value.core.uint8"
            | "value.core.uint16"
            | "value.core.uint32"
            | "value.core.uint64"
            | "value.core.bool"
    )
}

fn is_bang_message(message: &ControlMessage) -> bool {
    message.key == "bang" && message.atoms.is_empty()
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
            if message.key == "set" {
                return Some(ControlValue::string(set_message_text(message)));
            }
            if message.key == "symbol"
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
        "control input {node_id} expects {}, got message key {}",
        stored.kind_label(),
        message.key
    )
}

fn message_from_message_node_state(stored: &ControlValue) -> ControlMessage {
    match stored {
        ControlValue::String { value } => ControlMessage::parse_text(value),
        value => ControlMessage::from_value(value.clone()),
    }
}

fn set_message_text(message: &ControlMessage) -> String {
    if message.key == "set" {
        return message
            .atoms
            .iter()
            .map(control_atom_to_text)
            .collect::<Vec<_>>()
            .join(" ");
    }
    if message.key == "symbol"
        && let Some(ControlValue::String { value }) = message.first_atom()
    {
        return value.clone();
    }
    message.to_text()
}

fn silent_set_message(message: &ControlMessage) -> Option<String> {
    (message.key == "set").then(|| set_message_text(message))
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
    match message.key.as_str() {
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

fn operator_right_default(node: &GraphNode) -> Option<ControlValue> {
    if !is_control_operator_kind(&node.kind) || node.kind == OPERATOR_SQRT_KIND {
        return None;
    }
    Some(control_value_from_operator_param(node.params.get("right")))
}

fn control_value_from_operator_param(value: Option<&Value>) -> ControlValue {
    let Some(value) = value else {
        return ControlValue::float(0.0);
    };
    if let Some(value) = value.as_i64() {
        return ControlValue::int(value);
    }
    if let Some(value) = value.as_u64()
        && value <= i64::MAX as u64
    {
        return ControlValue::int(value as i64);
    }
    ControlValue::float(value.as_f64().unwrap_or(0.0))
}

fn numeric_message_value(message: &ControlMessage) -> Option<f64> {
    message.first_atom().and_then(control_value_as_f64)
}

fn control_value_as_f64(value: &ControlValue) -> Option<f64> {
    match value {
        ControlValue::Float { value, .. } => Some(sanitize_operator_number(*value)),
        ControlValue::Int { value, .. } => Some(*value as f64),
        ControlValue::Uint { value, .. } => Some(*value as f64),
        ControlValue::Bool { value } => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn evaluate_operator(kind: &str, input: f64, right: f64) -> f64 {
    let result = match kind {
        OPERATOR_ADD_KIND => input + right,
        OPERATOR_SUB_KIND => input - right,
        OPERATOR_MUL_KIND => input * right,
        OPERATOR_DIV_KIND => {
            if right == 0.0 {
                0.0
            } else {
                input / right
            }
        }
        OPERATOR_POW_KIND => input.powf(right),
        OPERATOR_MIN_KIND => input.min(right),
        OPERATOR_MAX_KIND => input.max(right),
        OPERATOR_SQRT_KIND => {
            if input < 0.0 {
                0.0
            } else {
                input.sqrt()
            }
        }
        _ => 0.0,
    };
    sanitize_operator_number(result)
}

fn sanitize_operator_number(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

fn operator_result_value(node: &GraphNode, value: f64) -> ControlValue {
    let data_kind = operator_result_data_kind(node);
    convert_control_value_to_data_kind(&ControlValue::float(value), data_kind, None)
        .unwrap_or_else(|| ControlValue::float(sanitize_operator_number(value)))
}

fn operator_result_data_kind(node: &GraphNode) -> &str {
    node.ports
        .iter()
        .find(|port| port.id == "out" && port.direction == PortDirection::Output)
        .map(|port| port.data_type.data_kind.as_str())
        .unwrap_or_else(
            || match control_value_from_operator_param(node.params.get("right")) {
                ControlValue::Int { .. } => "value.core.int32",
                _ => "value.core.float32",
            },
        )
}

#[cfg(test)]
mod tests;
