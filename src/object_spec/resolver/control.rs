use serde_json::{Map, Value, json};

use super::super::ports::{
    bang_ports, comment_ports, control_operator_ports, control_sqrt_ports, message_ports,
    stored_value_ports,
};
use super::super::{ObjectRegistryCandidate, ObjectSpecAtom, ObjectSpecResolution};
use super::atoms::{
    atom_display_text, insert_number, integer_value, numeric_value, unsigned_value,
};
use super::outcome::{failure, success};

pub(super) fn resolve_control_operator(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let kind = candidate.kind.as_str();
    if kind == "object.core.operator.sqrt" {
        if !creation_args.is_empty() {
            return failure(
                input,
                display_text,
                class_symbol,
                creation_args,
                "object-spec.invalid-arg-count",
                "sqrt accepts no creation arguments",
            );
        }
        return success(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            Map::new(),
            control_sqrt_ports(),
        );
    }

    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }

    let right = match creation_args.first() {
        Some(arg) => match numeric_value(arg) {
            Some(value) => value,
            None => {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-spec.invalid-arg-type",
                    format!("{class_symbol} creation argument must be numeric"),
                );
            }
        },
        None => 0.0,
    };
    let mut params = Map::new();
    insert_number(&mut params, "right", right);
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        candidate,
        params,
        control_operator_ports(),
    )
}

pub(super) fn resolve_control_value(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let kind = candidate.kind.as_str();
    match kind {
        "object.core.bang" => {
            if !creation_args.is_empty() {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-spec.invalid-arg-count",
                    format!("{class_symbol} accepts no creation arguments"),
                );
            }
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                candidate,
                Map::new(),
                bang_ports(),
            )
        }
        "object.core.message" | "object.core.comment" => {
            let text = creation_args
                .iter()
                .map(atom_display_text)
                .collect::<Vec<_>>()
                .join(" ");
            let mut params = Map::new();
            params.insert("text".to_owned(), Value::String(text));
            let ports = if kind == "object.core.message" {
                message_ports()
            } else {
                comment_ports()
            };
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                candidate,
                params,
                ports,
            )
        }
        "object.core.float" => resolve_number_value(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            NumberValueSpec {
                port_type: "value.core.float32",
                coerce: numeric_value,
                to_json: |value| json!(value),
            },
        ),
        "object.core.int" => resolve_number_value(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            NumberValueSpec {
                port_type: "value.core.int32",
                coerce: integer_value,
                to_json: |value| json!(value),
            },
        ),
        "object.core.uint" => resolve_number_value(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            NumberValueSpec {
                port_type: "value.core.uint32",
                coerce: unsigned_value,
                to_json: |value| json!(value),
            },
        ),
        _ => unreachable!("control value resolver received unknown kind"),
    }
}

struct NumberValueSpec<T> {
    port_type: &'static str,
    coerce: fn(&ObjectSpecAtom) -> Option<T>,
    to_json: fn(T) -> Value,
}

fn resolve_number_value<T>(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    spec: NumberValueSpec<T>,
) -> ObjectSpecResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }

    let value = match creation_args.first() {
        Some(arg) => match (spec.coerce)(arg) {
            Some(value) => (spec.to_json)(value),
            None => {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-spec.invalid-arg-type",
                    format!("{class_symbol} creation argument has the wrong numeric type"),
                );
            }
        },
        None => json!(0),
    };
    let mut params = Map::new();
    params.insert("value".to_owned(), value);
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        candidate,
        params,
        stored_value_ports(spec.port_type),
    )
}
