use serde_json::{Map, json};

use super::*;
use crate::{DataFlow, DataType, GraphNode, Port, PortActivation};

fn request(node_id: &str, port_id: &str, message: ControlMessage) -> RuntimeControlEventRequest {
    RuntimeControlEventRequest {
        node_id: node_id.to_owned(),
        port_id: port_id.to_owned(),
        message,
    }
}

fn value_request(node_id: &str, port_id: &str, value: ControlValue) -> RuntimeControlEventRequest {
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
            key: "set".to_owned(),
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
        value_node("rgba", COLOR_KIND, json!([0.1, 0.2, 0.3, 1.0])),
        value_node("message", MESSAGE_KIND, json!("perform")),
        value_node("slider", FLOAT_KIND, json!(0.75)),
        value_node("other", "debug.sink", json!(10)),
    ]));

    assert_eq!(state.values.len(), 5);
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
        state.value_for_node("message"),
        Some(&ControlValue::string("perform".to_owned()))
    );
    assert_eq!(
        state.value_for_node("slider"),
        Some(&ControlValue::float(0.75))
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
fn cold_typed_ports_reject_set_selector_messages() {
    let graph = graph(vec![
        value_node("value_1", FLOAT_KIND, json!(1.0)),
        operator_node("add_1", OPERATOR_ADD_KIND, Some(1.0)),
    ]);
    let mut state = ControlState::from_graph(&graph);

    let cold = state.apply_event(
        set_value_request("value_1", "cold", ControlValue::float(32.0)),
        &graph,
    );
    let right = state.apply_event(
        set_value_request("add_1", "right", ControlValue::float(2.0)),
        &graph,
    );

    assert!(!cold.ok);
    assert!(cold.issues[0].message.contains("value_1.cold"));
    assert!(!right.ok);
    assert!(right.issues[0].message.contains("add_1.right"));
    assert_eq!(
        state.value_for_node("value_1"),
        Some(&ControlValue::float(1.0))
    );
    assert_eq!(
        state.operator_right.get("add_1"),
        Some(&ControlValue::float(1.0))
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
fn numeric_controls_accept_bool_payloads_as_zero_or_one() {
    let graph = graph(vec![
        value_node("float_1", FLOAT_KIND, json!(0.5)),
        value_node("int_1", INT_KIND, json!(3)),
        value_node_with_representation("uint_1", INT_KIND, json!(4), "u32"),
    ]);
    let mut state = ControlState::from_graph(&graph);

    let float_response = state.apply_event(
        value_request("float_1", "in", ControlValue::bool(true)),
        &graph,
    );
    let int_response = state.apply_event(
        value_request("int_1", "cold", ControlValue::bool(false)),
        &graph,
    );
    let uint_response = state.apply_event(
        value_request("uint_1", "in", ControlValue::bool(true)),
        &graph,
    );

    assert!(float_response.ok);
    assert_eq!(
        emitted_value(&float_response.emitted[0]),
        Some(ControlValue::float(1.0))
    );
    assert_eq!(
        state.value_for_node("float_1"),
        Some(&ControlValue::float(1.0))
    );
    assert!(int_response.ok);
    assert!(int_response.emitted.is_empty());
    assert_eq!(state.value_for_node("int_1"), Some(&ControlValue::int(0)));
    assert!(uint_response.ok);
    assert_eq!(state.value_for_node("uint_1"), Some(&ControlValue::uint(1)));
}

#[test]
fn bang_emits_stored_value_without_update() {
    let graph = graph(vec![value_node("value_1", FLOAT_KIND, json!(1.25))]);
    let mut state = ControlState::from_graph(&graph);

    let response = state.apply_event(bang_request("value_1", "in"), &graph);

    assert!(response.ok);
    assert_eq!(
        response.emitted,
        vec![RuntimeControlEmission {
            node_id: "value_1".to_owned(),
            port_id: "value".to_owned(),
            message: ControlMessage::from_value(ControlValue::float(1.25))
        }]
    );
    assert_eq!(
        state.value_for_node("value_1"),
        Some(&ControlValue::float(1.25))
    );
}

#[test]
fn control_operator_hot_cold_and_bang_semantics() {
    let graph = graph(vec![operator_node("add_1", OPERATOR_ADD_KIND, Some(1.0))]);
    let mut state = ControlState::from_graph(&graph);

    assert_eq!(
        state.operator_right.get("add_1"),
        Some(&ControlValue::float(1.0))
    );

    let hot = state.apply_event(
        value_request("add_1", "in", ControlValue::float(4.0)),
        &graph,
    );
    assert!(hot.ok);
    assert_eq!(
        hot.emitted,
        vec![RuntimeControlEmission {
            node_id: "add_1".to_owned(),
            port_id: "out".to_owned(),
            message: ControlMessage::from_value(ControlValue::float(5.0))
        }]
    );
    assert_eq!(
        state.value_for_node("add_1"),
        Some(&ControlValue::float(5.0))
    );

    let cold = state.apply_event(
        value_request("add_1", "right", ControlValue::float(2.0)),
        &graph,
    );
    assert!(cold.ok);
    assert!(cold.emitted.is_empty());
    assert_eq!(
        state.operator_right.get("add_1"),
        Some(&ControlValue::float(2.0))
    );

    let silent_hot = state.apply_event(
        set_value_request("add_1", "in", ControlValue::float(4.0)),
        &graph,
    );
    assert!(silent_hot.ok);
    assert!(silent_hot.emitted.is_empty());
    assert_eq!(
        state.value_for_node("add_1"),
        Some(&ControlValue::float(6.0))
    );

    let bang = state.apply_event(bang_request("add_1", "in"), &graph);
    assert!(bang.ok);
    assert_eq!(
        bang.emitted,
        vec![RuntimeControlEmission {
            node_id: "add_1".to_owned(),
            port_id: "out".to_owned(),
            message: ControlMessage::from_value(ControlValue::float(6.0))
        }]
    );
}

#[test]
fn control_operator_deterministic_fallbacks() {
    let graph = graph(vec![
        operator_node("div_1", OPERATOR_DIV_KIND, Some(0.0)),
        operator_node("sqrt_1", OPERATOR_SQRT_KIND, None),
        operator_node("pow_1", OPERATOR_POW_KIND, Some(0.5)),
    ]);
    let mut state = ControlState::from_graph(&graph);

    let div = state.apply_event(
        value_request("div_1", "in", ControlValue::float(10.0)),
        &graph,
    );
    let sqrt = state.apply_event(
        value_request("sqrt_1", "in", ControlValue::float(-1.0)),
        &graph,
    );
    let pow = state.apply_event(
        value_request("pow_1", "in", ControlValue::float(-1.0)),
        &graph,
    );

    assert!(div.ok);
    assert!(sqrt.ok);
    assert!(pow.ok);
    assert_eq!(
        emitted_value(&div.emitted[0]),
        Some(ControlValue::float(0.0))
    );
    assert_eq!(
        emitted_value(&sqrt.emitted[0]),
        Some(ControlValue::float(0.0))
    );
    assert_eq!(
        emitted_value(&pow.emitted[0]),
        Some(ControlValue::float(0.0))
    );
}

#[test]
fn control_operator_accepts_integer_inputs_and_reports_invalid_messages() {
    let graph = graph(vec![
        operator_node("div_1", OPERATOR_DIV_KIND, Some(2.0)),
        operator_node("sqrt_1", OPERATOR_SQRT_KIND, None),
        operator_node("add_1", OPERATOR_ADD_KIND, Some(1.0)),
        operator_node("bool_add", OPERATOR_ADD_KIND, Some(4.0)),
    ]);
    let mut state = ControlState::from_graph(&graph);

    let div = state.apply_event(value_request("div_1", "in", ControlValue::int(9)), &graph);
    let sqrt = state.apply_event(value_request("sqrt_1", "in", ControlValue::uint(9)), &graph);
    let bool_hot = state.apply_event(
        value_request("bool_add", "in", ControlValue::bool(true)),
        &graph,
    );
    let bool_right = state.apply_event(
        value_request("bool_add", "right", ControlValue::bool(false)),
        &graph,
    );
    let bad_hot = state.apply_event(
        RuntimeControlEventRequest {
            node_id: "add_1".to_owned(),
            port_id: "in".to_owned(),
            message: ControlMessage::from_value(ControlValue::string("bad")),
        },
        &graph,
    );
    let bad_right_bang = state.apply_event(bang_request("add_1", "right"), &graph);
    let bad_right_value = state.apply_event(
        RuntimeControlEventRequest {
            node_id: "add_1".to_owned(),
            port_id: "right".to_owned(),
            message: ControlMessage::from_value(ControlValue::string("bad")),
        },
        &graph,
    );
    let bad_port = state.apply_event(
        value_request("add_1", "missing", ControlValue::float(1.0)),
        &graph,
    );
    let add_node = graph
        .nodes
        .iter()
        .find(|node| node.id == "add_1")
        .expect("operator node should exist");
    let bad_internal_port = state.apply_operator_event(
        add_node,
        "missing",
        ControlMessage::from_value(ControlValue::float(1.0)),
    );

    assert!(div.ok);
    assert!(sqrt.ok);
    assert_eq!(
        emitted_value(&div.emitted[0]),
        Some(ControlValue::float(4.5))
    );
    assert_eq!(
        emitted_value(&sqrt.emitted[0]),
        Some(ControlValue::float(3.0))
    );
    assert!(bool_hot.ok);
    assert_eq!(
        emitted_value(&bool_hot.emitted[0]),
        Some(ControlValue::float(5.0))
    );
    assert!(bool_right.ok);
    assert_eq!(
        state.operator_right.get("bool_add"),
        Some(&ControlValue::bool(false))
    );
    assert!(!bad_hot.ok);
    assert!(!bad_right_bang.ok);
    assert!(!bad_right_value.ok);
    assert!(!bad_port.ok);
    assert!(!bad_internal_port.ok);
    assert_eq!(
        state.value_for_node("add_1"),
        Some(&ControlValue::float(0.0))
    );
    assert_eq!(
        state.operator_right.get("add_1"),
        Some(&ControlValue::float(1.0))
    );
    assert_eq!(
        evaluate_operator("object.core.operator.unknown", 2.0, 3.0),
        0.0
    );
}

#[test]
fn control_operator_output_type_follows_resolved_literal_not_input_type() {
    let graph = graph(vec![
        operator_node_with_output_type("mul_int", OPERATOR_MUL_KIND, json!(3), "value.core.int32"),
        operator_node_with_output_type(
            "mul_float",
            OPERATOR_MUL_KIND,
            json!(3.0),
            "value.core.float32",
        ),
    ]);
    let mut state = ControlState::from_graph(&graph);

    let int_operator_float_input = state.apply_event(
        value_request("mul_int", "in", ControlValue::float(2.5)),
        &graph,
    );
    let float_operator_int_input = state.apply_event(
        value_request("mul_float", "in", ControlValue::int(2)),
        &graph,
    );
    let int_operator_float_right = state.apply_event(
        value_request("mul_int", "right", ControlValue::float(4.5)),
        &graph,
    );
    let int_operator_after_float_right =
        state.apply_event(value_request("mul_int", "in", ControlValue::int(2)), &graph);

    assert!(int_operator_float_input.ok);
    assert_eq!(
        emitted_value(&int_operator_float_input.emitted[0]),
        Some(ControlValue::int(7))
    );
    assert!(float_operator_int_input.ok);
    assert_eq!(
        emitted_value(&float_operator_int_input.emitted[0]),
        Some(ControlValue::float(6.0))
    );
    assert_eq!(
        state.value_for_node("mul_float"),
        Some(&ControlValue::float(6.0))
    );
    assert!(int_operator_float_right.ok);
    assert_eq!(
        state.operator_right.get("mul_int"),
        Some(&ControlValue::float(4.5))
    );
    assert!(int_operator_after_float_right.ok);
    assert_eq!(
        emitted_value(&int_operator_after_float_right.emitted[0]),
        Some(ControlValue::int(9))
    );
    assert_eq!(state.value_for_node("mul_int"), Some(&ControlValue::int(9)));
}

#[test]
fn control_operator_uses_fallbacks_for_missing_runtime_slots() {
    let graph = graph(vec![operator_node("add_1", OPERATOR_ADD_KIND, Some(2.0))]);
    let mut state = ControlState::from_graph(&graph);

    state.values.remove("add_1");
    let bang = state.apply_event(bang_request("add_1", "in"), &graph);
    assert!(bang.ok);
    assert_eq!(
        emitted_value(&bang.emitted[0]),
        Some(ControlValue::float(0.0))
    );

    state.operator_right.remove("add_1");
    let numeric = state.apply_event(
        value_request("add_1", "in", ControlValue::float(3.0)),
        &graph,
    );
    assert!(numeric.ok);
    assert_eq!(
        emitted_value(&numeric.emitted[0]),
        Some(ControlValue::float(5.0))
    );
}

#[test]
fn control_operator_edges_propagate_results() {
    let mut graph = graph(vec![
        value_node("source_1", FLOAT_KIND, json!(4.0)),
        operator_node("mul_1", OPERATOR_MUL_KIND, Some(0.5)),
        value_node("target_1", FLOAT_KIND, json!(0.0)),
    ]);
    graph.edges = vec![
        edge("source_1", "value", "mul_1", "in"),
        edge("mul_1", "out", "target_1", "in"),
    ];
    let mut state = ControlState::from_graph(&graph);

    let response = state.apply_event(
        value_request("source_1", "in", ControlValue::float(8.0)),
        &graph,
    );

    assert!(response.ok);
    assert_eq!(
        state.value_for_node("mul_1"),
        Some(&ControlValue::float(4.0))
    );
    assert_eq!(
        state.value_for_node("target_1"),
        Some(&ControlValue::float(4.0))
    );
    assert_eq!(
        response
            .emitted
            .iter()
            .map(|emission| (emission.node_id.as_str(), emission.port_id.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("source_1", "value"),
            ("mul_1", "out"),
            ("target_1", "value")
        ]
    );
}

#[test]
fn control_edges_convert_bool_payloads_for_numeric_targets() {
    let mut graph = graph(vec![
        value_node("message_1", MESSAGE_KIND, json!("bool true")),
        value_node("float_1", FLOAT_KIND, json!(0.0)),
        value_node("int_1", INT_KIND, json!(0)),
    ]);
    graph.edges = vec![
        edge("message_1", "out", "float_1", "in"),
        edge("float_1", "value", "int_1", "in"),
    ];
    let mut state = ControlState::from_graph(&graph);

    let response = state.apply_event(bang_request("message_1", "in"), &graph);

    assert!(response.ok, "{:?}", response.issues);
    assert_eq!(
        state.value_for_node("float_1"),
        Some(&ControlValue::float(1.0))
    );
    assert_eq!(state.value_for_node("int_1"), Some(&ControlValue::int(1)));
}

#[test]
fn message_controls_emit_strings() {
    let graph = graph(vec![value_node(
        "message_1",
        MESSAGE_KIND,
        json!("perform"),
    )]);
    let mut state = ControlState::from_graph(&graph);

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
        cold_message.issues[0]
            .message
            .contains("does not support runtime control input port cold")
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
        state.channels.get("value.core.float32:speed"),
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
        state.channels.get("value.core.bang:go"),
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
    let mut uint_receiver =
        value_node_with_representation("uint_receiver", INT_KIND, json!(0), "u32");
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
    assert!(response.issues.is_empty(), "{:?}", response.issues);
    assert_eq!(
        state.value_for_node("int_receiver"),
        Some(&ControlValue::int(12))
    );
    assert_eq!(
        state.value_for_node("uint_receiver"),
        Some(&ControlValue::uint(12))
    );

    let mut bool_sender = value_node("bool_sender", MESSAGE_KIND, json!("bool true"));
    bool_sender
        .params
        .insert("sendName".to_owned(), json!("gate"));
    let mut float_receiver = value_node("float_receiver", FLOAT_KIND, json!(0.0));
    float_receiver
        .params
        .insert("receiveName".to_owned(), json!("gate"));
    let bool_routing_graph = graph(vec![bool_sender, float_receiver]);
    let mut bool_state = ControlState::from_graph(&bool_routing_graph);

    let bool_response =
        bool_state.apply_event(bang_request("bool_sender", "in"), &bool_routing_graph);

    assert!(bool_response.ok, "{:?}", bool_response.issues);
    assert_eq!(
        bool_state.value_for_node("float_receiver"),
        Some(&ControlValue::float(1.0))
    );
}

#[test]
fn panel_inlet_accepts_set_message_silently() {
    let graph = graph(vec![panel_node("panel_1")]);
    let mut state = ControlState::from_graph(&graph);

    let response = state.apply_event(
        request("panel_1", "in", ControlMessage::parse_text("set #00ff00")),
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
fn comment_inlet_accepts_set_message_silently() {
    let graph = graph(vec![comment_node("comment_1", "old text")]);
    let mut state = ControlState::from_graph(&graph);

    let response = state.apply_event(
        request(
            "comment_1",
            "in",
            ControlMessage::parse_text("set updated note"),
        ),
        &graph,
    );

    assert!(response.ok);
    assert!(response.emitted.is_empty());
    assert_eq!(
        state.value_for_node("comment_1"),
        Some(&ControlValue::string("updated note".to_owned()))
    );
}

#[test]
fn comment_and_panel_inlets_reject_non_set_messages() {
    let graph = graph(vec![
        comment_node("comment_1", "old text"),
        panel_node("panel_1"),
    ]);
    let mut state = ControlState::from_graph(&graph);

    let comment_response = state.apply_event(
        request("comment_1", "in", ControlMessage::parse_text("ignored")),
        &graph,
    );
    let panel_response = state.apply_event(bang_request("panel_1", "in"), &graph);

    assert!(!comment_response.ok);
    assert!(comment_response.emitted.is_empty());
    assert!(
        comment_response.issues[0]
            .message
            .contains("expects set message")
    );
    assert!(!panel_response.ok);
    assert!(panel_response.emitted.is_empty());
    assert!(
        panel_response.issues[0]
            .message
            .contains("expects set message")
    );
    assert_eq!(
        state.value_for_node("comment_1"),
        Some(&ControlValue::string("old text".to_owned()))
    );
    assert_eq!(
        state.value_for_node("panel_1"),
        Some(&ControlValue::string("transparent".to_owned()))
    );
}

#[test]
fn object_receive_name_dispatches_set_messages_to_panel_inlet() {
    let mut sender = value_node("message_1", MESSAGE_KIND, json!("set #00ff00"));
    sender.params.insert("sendName".to_owned(), json!("status"));
    let mut receiver = panel_node("panel_1");
    receiver
        .params
        .insert("receiveName".to_owned(), json!("status"));
    let routing_graph = graph(vec![sender, receiver]);
    let mut state = ControlState::from_graph(&routing_graph);

    let response = state.apply_event(bang_request("message_1", "in"), &routing_graph);

    assert!(response.ok);
    assert_eq!(
        response.emitted,
        vec![RuntimeControlEmission {
            node_id: "message_1".to_owned(),
            port_id: "out".to_owned(),
            message: ControlMessage::parse_text("set #00ff00"),
        }]
    );
    assert_eq!(
        state.value_for_node("panel_1"),
        Some(&ControlValue::string("#00ff00".to_owned()))
    );
}

#[test]
fn message_set_updates_comment_and_panel_through_inlets() {
    let mut graph = graph(vec![
        bang_node("button_1"),
        value_node("message_1", MESSAGE_KIND, json!("set hello world")),
        comment_node("comment_1", "old comment"),
        value_node("message_2", MESSAGE_KIND, json!("set #00ff00")),
        panel_node("panel_1"),
    ]);
    graph.edges = vec![
        edge("button_1", "out", "message_1", "in"),
        edge("message_1", "out", "comment_1", "in"),
        edge("button_1", "out", "message_2", "in"),
        edge("message_2", "out", "panel_1", "in"),
    ];
    let mut state = ControlState::from_graph(&graph);

    let response = state.apply_event(bang_request("button_1", "in"), &graph);

    assert!(response.ok, "{:?}", response.issues);
    assert_eq!(
        state.value_for_node("comment_1"),
        Some(&ControlValue::string("hello world".to_owned()))
    );
    assert_eq!(
        state.value_for_node("panel_1"),
        Some(&ControlValue::string("#00ff00".to_owned()))
    );
}

#[test]
fn object_channel_helpers_skip_missing_sources_empty_names_and_mismatched_receivers() {
    let mut sender = value_node("slider_1", FLOAT_KIND, json!(0.25));
    sender.params.insert("sendName".to_owned(), json!("   "));
    let mut wrong_receiver = value_node("color_1", COLOR_KIND, json!([0.0, 0.0, 0.0, 1.0]));
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
        state.value_for_node("color_1"),
        Some(&ControlValue::color([0.0, 0.0, 0.0, 1.0]))
    );

    let mut sender = value_node("slider_2", FLOAT_KIND, json!(0.25));
    sender.params.insert("sendName".to_owned(), json!("speed"));
    let mut wrong_receiver = value_node("color_2", COLOR_KIND, json!([0.0, 0.0, 0.0, 1.0]));
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
    assert_eq!(mismatched.issues.len(), 1);
    assert!(
        mismatched.issues[0]
            .message
            .contains("ignored incompatible routed value.core.float32")
    );
    assert_eq!(
        mismatched_state.value_for_node("color_2"),
        Some(&ControlValue::color([0.0, 0.0, 0.0, 1.0]))
    );

    let mut sender = value_node("string_sender", MESSAGE_KIND, json!("symbol new"));
    sender.params.insert("sendName".to_owned(), json!("label"));
    let mut broken_receiver = comment_node("comment_receiver", "old");
    broken_receiver
        .params
        .insert("receiveName".to_owned(), json!("label"));
    let rejected_receiver_graph = graph(vec![sender, broken_receiver]);
    let mut rejected_state = ControlState::from_graph(&rejected_receiver_graph);
    let rejected = rejected_state.publish_object_channel(
        &RuntimeControlEmission {
            node_id: "string_sender".to_owned(),
            port_id: "out".to_owned(),
            message: ControlMessage::from_value(ControlValue::string("new".to_owned())),
        },
        &rejected_receiver_graph,
    );
    assert!(rejected.ok);
    assert_eq!(rejected.issues.len(), 1);
    assert!(
        rejected.issues[0]
            .message
            .contains("rejected routed value.core.string")
    );
    assert_eq!(
        rejected_state.value_for_node("comment_receiver"),
        Some(&ControlValue::string("old".to_owned()))
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
        port_type_for_control_value(&ControlValue::int(1)),
        "value.core.int32"
    );
    assert_eq!(
        port_type_for_control_value(&ControlValue::uint(1)),
        "value.core.uint32"
    );
    assert_eq!(
        port_type_for_control_value(&ControlValue::color([1.0, 0.0, 0.0, 1.0])),
        "value.core.color"
    );
    assert!(object_accepts_data_kind(
        &value_node("i32_1", INT_KIND, json!(0)),
        "value.core.int32"
    ));
    assert!(object_accepts_data_kind(
        &value_node("f32_1", FLOAT_KIND, json!(0.0)),
        "value.core.bool"
    ));
    assert!(object_accepts_data_kind(
        &value_node("i32_1", INT_KIND, json!(0)),
        "value.core.bool"
    ));
    assert!(object_accepts_data_kind(
        &value_node_with_representation("u32_1", INT_KIND, json!(0), "u32"),
        "value.core.bool"
    ));
    assert!(object_accepts_data_kind(
        &value_node_with_representation("u32_1", INT_KIND, json!(0), "u32"),
        "value.core.uint32"
    ));
    assert!(object_accepts_data_kind(
        &value_node("rgba_1", COLOR_KIND, json!([1.0, 0.0, 0.0, 1.0])),
        "value.core.color"
    ));
    assert!(object_accepts_data_kind(
        &value_node("message_1", MESSAGE_KIND, json!("go")),
        "value.core.string"
    ));
    assert!(object_accepts_data_kind(
        &value_node("message_1", MESSAGE_KIND, json!("go")),
        "value.core.float32"
    ));
    assert!(object_accepts_data_kind(
        &bang_node("button_1"),
        "value.core.bang"
    ));
    assert!(object_accepts_data_kind(
        &bang_node("button_1"),
        "value.core.float32"
    ));
    assert!(object_accepts_data_kind(
        &bang_node("button_1"),
        "value.core.string"
    ));
    for data_kind in [
        "value.core.float32",
        "value.core.int32",
        "value.core.uint32",
        "value.core.bool",
        "value.core.color",
        "value.core.string",
        "value.core.bang",
        "value.core.message",
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
    assert!(response.issues[0].message.contains("port missing"));
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
    assert!(response.issues[0].message.contains("runtime safety limit"));
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

    let button = state.apply_event(bang_request("button_1", "in"), &graph);
    assert!(button.ok);
    assert_eq!(button.emitted[0].message, ControlMessage::bang());
}

#[test]
fn ui_panel_controls_reject_wrong_ports_and_types() {
    let graph = graph(vec![
        value_node("slider_1", FLOAT_KIND, json!(0.5)),
        bang_node("button_1"),
    ]);
    let mut state = ControlState::from_graph(&graph);

    let wrong_button_port = state.apply_event(bang_request("button_1", "value"), &graph);
    assert!(!wrong_button_port.ok);
    assert!(wrong_button_port.emitted.is_empty());

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

    let slider_bool_cold = state.apply_event(
        value_request("slider_1", "cold", ControlValue::bool(false)),
        &graph,
    );
    assert!(slider_bool_cold.ok);
    assert!(slider_bool_cold.emitted.is_empty());
    assert_eq!(
        state.value_for_node("slider_1"),
        Some(&ControlValue::float(0.0))
    );

    let slider_bool_hot = state.apply_event(
        value_request("slider_1", "in", ControlValue::bool(true)),
        &graph,
    );
    assert!(slider_bool_hot.ok);
    assert_eq!(
        emitted_value(&slider_bool_hot.emitted[0]),
        Some(ControlValue::float(1.0))
    );

    let slider_bang = state.apply_event(bang_request("slider_1", "in"), &graph);
    assert!(slider_bang.ok);
    assert_eq!(
        emitted_value(&slider_bang.emitted[0]),
        Some(ControlValue::float(1.0))
    );

    let slider_bool = state.apply_event(
        value_request("slider_1", "in", ControlValue::bool(true)),
        &graph,
    );
    assert!(slider_bool.ok);
    assert_eq!(
        state.value_for_node("slider_1"),
        Some(&ControlValue::float(1.0))
    );

    let slider_other = state.apply_event(
        value_request("slider_1", "other", ControlValue::float(1.0)),
        &graph,
    );
    assert!(!slider_other.ok);

    state.values.remove("slider_1");
    let slider_missing_state = state.apply_event(bang_request("slider_1", "in"), &graph);
    assert!(!slider_missing_state.ok);
    assert!(
        slider_missing_state.issues[0]
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
        "value.core.float32:speed".to_owned(),
        ControlMessage::from_value(ControlValue::float(1.5)),
    );

    let response = RuntimeControlStateResponse {
        ok: true,
        control_revision: 7,
        values,
        channels,
        issues: Vec::new(),
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
                "value.core.float32:speed": {
                    "key": "float",
                    "atoms": [{ "type": "float", "representation": "f32", "value": 1.5 }]
                }
            },
            "issues": []
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
        ControlState::from_graph(&graph(vec![node.clone()])).output_value_for_node(&node, "value",),
        Some(ControlValue::float(0.5))
    );
    assert_eq!(
        ControlState::from_graph(&graph(vec![node.clone()])).output_value_for_node(&node, "other",),
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
        key: "clear".to_owned(),
        atoms: Vec::new(),
    };
    assert_eq!(
        data_kind_for_control_message(&selector_only),
        "value.core.message"
    );
    assert_eq!(set_message_text(&selector_only), "clear");
    assert_eq!(
        set_message_text(&ControlMessage::from_value(ControlValue::string(
            "hello".to_owned()
        ))),
        "hello"
    );
    assert_eq!(
        set_message_text(&ControlMessage {
            key: "set".to_owned(),
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
                key: "0".to_owned(),
                atoms: Vec::new()
            },
            true
        ),
        Some(false)
    );
    assert_eq!(
        coerce_toggle_input(
            &ControlMessage {
                key: "1".to_owned(),
                atoms: Vec::new()
            },
            false
        ),
        Some(true)
    );
    assert_eq!(
        coerce_toggle_input(
            &ControlMessage {
                key: "pulse".to_owned(),
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
        response.issues[0]
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
        value_request("value_1", "cold", ControlValue::color([0.0, 0.0, 0.0, 1.0])),
        bang_request("value_1", "cold"),
    ] {
        let response = state.apply_event(request, &graph);
        assert!(!response.ok);
        assert!(response.emitted.is_empty());
        assert!(!response.issues.is_empty());
        assert_eq!(
            state.value_for_node("value_1"),
            Some(&ControlValue::float(1.0))
        );
    }
}

#[test]
fn rejects_existing_unsupported_input_ports() {
    let mut graph = graph(vec![
        value_node("value_1", FLOAT_KIND, json!(1.0)),
        value_node("message_1", MESSAGE_KIND, json!("go")),
    ]);
    graph.nodes[0].ports.push(port(
        "set",
        PortDirection::Input,
        DataFlow::Control,
        "value.core.message",
        Some(PortActivation::Trigger),
    ));
    graph.nodes[1].ports.push(port(
        "cold",
        PortDirection::Input,
        DataFlow::Control,
        "value.core.message",
        Some(PortActivation::Latched),
    ));
    let mut state = ControlState::from_graph(&graph);

    let unsupported_set = state.apply_event(
        request("value_1", "set", ControlMessage::parse_text("set 2")),
        &graph,
    );
    assert!(!unsupported_set.ok);
    assert!(
        unsupported_set.issues[0]
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
        unsupported_message_cold.issues[0]
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
        non_control.issues[0]
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
        missing_state.issues[0]
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
        FLOAT_KIND => stored_value_ports("value.core.float32"),
        INT_KIND => stored_value_ports("value.core.int32"),
        COLOR_KIND => stored_value_ports("color"),
        MESSAGE_KIND => message_ports(),
        _ => Vec::new(),
    };
    if id.contains("slider") {
        params.insert("widget".to_owned(), json!("slider"));
    }
    GraphNode {
        id: id.to_owned(),
        kind: kind.to_owned(),
        kind_version: "0.1.0".to_owned(),
        params,
        ports,
    }
}

fn value_node_with_representation(
    id: &str,
    kind: &str,
    value: serde_json::Value,
    representation: &str,
) -> GraphNode {
    let mut node = value_node(id, kind, value);
    node.params
        .insert("representation".to_owned(), json!(representation));
    if kind == INT_KIND && representation.starts_with('u') {
        node.ports = stored_value_ports("value.core.uint32");
    }
    node
}

fn operator_node(id: &str, kind: &str, right: Option<f64>) -> GraphNode {
    let mut params = Map::new();
    if let Some(right) = right {
        params.insert("right".to_owned(), json!(right));
    }
    operator_node_with_params_and_output_type(id, kind, params, "value.core.float32")
}

fn operator_node_with_output_type(
    id: &str,
    kind: &str,
    right: serde_json::Value,
    output_type: &str,
) -> GraphNode {
    let mut params = Map::new();
    params.insert("right".to_owned(), right);
    operator_node_with_params_and_output_type(id, kind, params, output_type)
}

fn operator_node_with_params_and_output_type(
    id: &str,
    kind: &str,
    params: Map<String, serde_json::Value>,
    output_type: &str,
) -> GraphNode {
    let mut ports = vec![port(
        "in",
        PortDirection::Input,
        DataFlow::Control,
        "value.core.message",
        Some(PortActivation::Trigger),
    )];
    if kind != OPERATOR_SQRT_KIND {
        ports.push(port(
            "right",
            PortDirection::Input,
            DataFlow::Control,
            "value.core.message",
            Some(PortActivation::Latched),
        ));
    }
    ports.push(port(
        "out",
        PortDirection::Output,
        DataFlow::Control,
        output_type,
        None,
    ));
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
            "in",
            PortDirection::Input,
            DataFlow::Control,
            "value.core.message",
            Some(PortActivation::Trigger),
        )],
    }
}

fn comment_node(id: &str, text: &str) -> GraphNode {
    let mut params = Map::new();
    params.insert("text".to_owned(), json!(text));
    GraphNode {
        id: id.to_owned(),
        kind: COMMENT_KIND.to_owned(),
        kind_version: "0.1.0".to_owned(),
        params,
        ports: vec![port(
            "in",
            PortDirection::Input,
            DataFlow::Control,
            "value.core.message",
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
                DataFlow::Control,
                "value.core.message",
                Some(PortActivation::Trigger),
            ),
            port(
                "out",
                PortDirection::Output,
                DataFlow::Event,
                "value.core.bang",
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
            DataFlow::Control,
            "value.core.message",
            Some(PortActivation::Trigger),
        ),
        port(
            "cold",
            PortDirection::Input,
            DataFlow::Control,
            data_kind,
            Some(PortActivation::Latched),
        ),
        port(
            "value",
            PortDirection::Output,
            DataFlow::Control,
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
            DataFlow::Control,
            "value.core.message",
            Some(PortActivation::Trigger),
        ),
        port(
            "out",
            PortDirection::Output,
            DataFlow::Control,
            "value.core.message",
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
