use skenion_contracts::MessageKeyPolicyV01;

use super::{
    ObjectSpecPort, ObjectSpecPortActivation, ObjectSpecPortDirection, ObjectSpecPortRate,
};

pub(super) fn input_port(
    id: &str,
    port_type: &str,
    rate: ObjectSpecPortRate,
    activation: ObjectSpecPortActivation,
) -> ObjectSpecPort {
    ObjectSpecPort {
        id: id.to_owned(),
        direction: ObjectSpecPortDirection::Input,
        port_type: port_type.to_owned(),
        label: None,
        rate,
        accepts: None,
        activation: Some(activation),
        message_keys: None,
    }
}

fn output_port(id: &str, port_type: &str, rate: ObjectSpecPortRate) -> ObjectSpecPort {
    ObjectSpecPort {
        id: id.to_owned(),
        direction: ObjectSpecPortDirection::Output,
        port_type: port_type.to_owned(),
        label: None,
        rate,
        accepts: None,
        activation: None,
        message_keys: None,
    }
}

fn with_accepts(mut port: ObjectSpecPort, accepts: &[&str]) -> ObjectSpecPort {
    port.accepts = Some(string_list(accepts));
    port
}

fn with_message_keys(mut port: ObjectSpecPort, policy: MessageKeyPolicyV01) -> ObjectSpecPort {
    port.message_keys = Some(policy);
    port
}

fn message_input_port(
    id: &str,
    activation: ObjectSpecPortActivation,
    accepts: &[&str],
    policy: MessageKeyPolicyV01,
) -> ObjectSpecPort {
    with_message_keys(
        with_accepts(
            input_port(
                id,
                "value.core.message",
                ObjectSpecPortRate::Control,
                activation,
            ),
            accepts,
        ),
        policy,
    )
}

pub(super) fn message_key_policy(
    accepted: &[&str],
    silent: &[&str],
    trigger: &[&str],
    store: &[&str],
    emit: &[&str],
) -> MessageKeyPolicyV01 {
    MessageKeyPolicyV01 {
        accepted: string_list(accepted),
        silent: optional_string_list(silent),
        trigger: optional_string_list(trigger),
        store: optional_string_list(store),
        emit: optional_string_list(emit),
    }
}

fn string_list(values: &[&str]) -> Vec<String> {
    unique_strings(values.iter().map(|value| (*value).to_owned()))
}

pub(super) fn unique_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        if unique.iter().any(|existing| existing == &value) {
            continue;
        }
        unique.push(value);
    }
    unique
}

fn optional_string_list(values: &[&str]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| string_list(values))
}

fn numeric_message_input_policy() -> MessageKeyPolicyV01 {
    message_key_policy(
        &["bang", "set", "float", "int", "uint", "bool"],
        &["set"],
        &["bang", "float", "int", "uint", "bool"],
        &["set", "float", "int", "uint", "bool"],
        &["bang", "float", "int", "uint", "bool"],
    )
}

fn bang_message_input_policy() -> MessageKeyPolicyV01 {
    message_key_policy(&["bang"], &[], &["bang"], &[], &["bang"])
}

fn stored_message_input_policy() -> MessageKeyPolicyV01 {
    message_key_policy(
        &["bang", "set", "float", "int", "uint", "bool", "message"],
        &["set"],
        &["bang", "float", "int", "uint", "bool", "message"],
        &["set", "float", "int", "uint", "bool", "message"],
        &["bang", "float", "int", "uint", "bool", "message"],
    )
}

fn comment_message_input_policy() -> MessageKeyPolicyV01 {
    message_key_policy(
        &["set", "float", "int", "uint", "bool", "message"],
        &["set"],
        &["float", "int", "uint", "bool", "message"],
        &["set", "float", "int", "uint", "bool", "message"],
        &[],
    )
}

pub(super) fn stored_value_ports(port_type: &str) -> Vec<ObjectSpecPort> {
    vec![
        message_input_port(
            "in",
            ObjectSpecPortActivation::Trigger,
            numeric_message_value_types(),
            numeric_message_input_policy(),
        ),
        input_port(
            "cold",
            port_type,
            ObjectSpecPortRate::Control,
            ObjectSpecPortActivation::Latched,
        ),
        output_port("value", port_type, ObjectSpecPortRate::Control),
    ]
}

pub(super) fn numeric_message_value_types() -> &'static [&'static str] {
    &[
        "value.core.float8",
        "value.core.float16",
        "value.core.float32",
        "value.core.float64",
        "value.core.ufloat8",
        "value.core.ufloat16",
        "value.core.ufloat32",
        "value.core.ufloat64",
        "value.core.int8",
        "value.core.int16",
        "value.core.int32",
        "value.core.int64",
        "value.core.uint8",
        "value.core.uint16",
        "value.core.uint32",
        "value.core.uint64",
        "value.core.bool",
        "value.core.bang",
    ]
}

fn numeric_latched_value_types() -> &'static [&'static str] {
    &[
        "value.core.float8",
        "value.core.float16",
        "value.core.float32",
        "value.core.float64",
        "value.core.ufloat8",
        "value.core.ufloat16",
        "value.core.ufloat32",
        "value.core.ufloat64",
        "value.core.int8",
        "value.core.int16",
        "value.core.int32",
        "value.core.int64",
        "value.core.uint8",
        "value.core.uint16",
        "value.core.uint32",
        "value.core.uint64",
        "value.core.bool",
    ]
}

fn control_message_value_types() -> &'static [&'static str] {
    &[
        "value.core.float8",
        "value.core.float16",
        "value.core.float32",
        "value.core.float64",
        "value.core.ufloat8",
        "value.core.ufloat16",
        "value.core.ufloat32",
        "value.core.ufloat64",
        "value.core.int8",
        "value.core.int16",
        "value.core.int32",
        "value.core.int64",
        "value.core.uint8",
        "value.core.uint16",
        "value.core.uint32",
        "value.core.uint64",
        "value.core.bool",
        "value.core.bang",
        "value.core.message",
    ]
}

fn numeric_latched_input_policy() -> MessageKeyPolicyV01 {
    message_key_policy(
        &["float", "int", "uint", "bool"],
        &[],
        &["float", "int", "uint", "bool"],
        &["float", "int", "uint", "bool"],
        &[],
    )
}

pub(super) fn control_operator_ports(output_type: &str) -> Vec<ObjectSpecPort> {
    vec![
        message_input_port(
            "in",
            ObjectSpecPortActivation::Trigger,
            numeric_message_value_types(),
            numeric_message_input_policy(),
        ),
        message_input_port(
            "right",
            ObjectSpecPortActivation::Latched,
            numeric_latched_value_types(),
            numeric_latched_input_policy(),
        ),
        output_port("out", output_type, ObjectSpecPortRate::Control),
    ]
}

pub(super) fn control_sqrt_ports() -> Vec<ObjectSpecPort> {
    vec![
        message_input_port(
            "in",
            ObjectSpecPortActivation::Trigger,
            numeric_message_value_types(),
            numeric_message_input_policy(),
        ),
        output_port("out", "value.core.float32", ObjectSpecPortRate::Control),
    ]
}

pub(super) fn bang_ports() -> Vec<ObjectSpecPort> {
    vec![
        message_input_port(
            "in",
            ObjectSpecPortActivation::Trigger,
            &["value.core.bang"],
            bang_message_input_policy(),
        ),
        output_port("out", "value.core.bang", ObjectSpecPortRate::Event),
    ]
}

pub(super) fn message_ports() -> Vec<ObjectSpecPort> {
    vec![
        message_input_port(
            "in",
            ObjectSpecPortActivation::Trigger,
            control_message_value_types(),
            stored_message_input_policy(),
        ),
        output_port("out", "value.core.message", ObjectSpecPortRate::Control),
    ]
}

pub(super) fn comment_ports() -> Vec<ObjectSpecPort> {
    vec![message_input_port(
        "in",
        ObjectSpecPortActivation::Trigger,
        control_message_value_types(),
        comment_message_input_policy(),
    )]
}

pub(super) fn audio_sig_ports() -> Vec<ObjectSpecPort> {
    vec![
        input_port(
            "value",
            "value.core.float32",
            ObjectSpecPortRate::Control,
            ObjectSpecPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectSpecPortRate::Audio),
    ]
}

pub(super) fn audio_osc_ports() -> Vec<ObjectSpecPort> {
    vec![
        input_port(
            "frequency",
            "value.core.float32",
            ObjectSpecPortRate::Control,
            ObjectSpecPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectSpecPortRate::Audio),
    ]
}

pub(super) fn audio_binary_ports() -> Vec<ObjectSpecPort> {
    vec![
        input_port(
            "left",
            "value.core.float32",
            ObjectSpecPortRate::Audio,
            ObjectSpecPortActivation::Latched,
        ),
        input_port(
            "right",
            "value.core.float32",
            ObjectSpecPortRate::Audio,
            ObjectSpecPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectSpecPortRate::Audio),
    ]
}

pub(super) fn audio_input_ports() -> Vec<ObjectSpecPort> {
    vec![
        output_port("left", "value.core.float32", ObjectSpecPortRate::Audio),
        output_port("right", "value.core.float32", ObjectSpecPortRate::Audio),
    ]
}

pub(super) fn audio_output_ports() -> Vec<ObjectSpecPort> {
    vec![
        input_port(
            "left",
            "value.core.float32",
            ObjectSpecPortRate::Audio,
            ObjectSpecPortActivation::Latched,
        ),
        input_port(
            "right",
            "value.core.float32",
            ObjectSpecPortRate::Audio,
            ObjectSpecPortActivation::Latched,
        ),
    ]
}
