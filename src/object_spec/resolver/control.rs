use serde_json::{Map, Value, json};

use crate::control_value::{
    value_type_id_for_float_representation, value_type_id_for_int_representation,
};

use super::super::ports::{
    bang_ports, comment_ports, control_operator_ports, control_sqrt_ports, message_ports,
    stored_value_ports,
};
use super::super::{ObjectRegistryCandidate, ObjectSpecAtom, ObjectSpecResolution};
use super::atoms::{atom_display_text, insert_number, integer_value, numeric_value};
use super::outcome::{failure, success};

pub(super) fn resolve_control_operator(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let kind = candidate.executable_kind.as_str();
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
    let kind = candidate.executable_kind.as_str();
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
            NumberValueKind::Float,
        ),
        "object.core.int" => resolve_number_value(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            NumberValueKind::Int,
        ),
        _ => unreachable!("control value resolver received unknown kind"),
    }
}

enum NumberValueKind {
    Float,
    Int,
}

fn resolve_number_value(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    kind: NumberValueKind,
) -> ObjectSpecResolution {
    if creation_args.len() > 2 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }

    let (representation, value_arg) = match parse_number_representation_arg(&creation_args, &kind) {
        Ok(parsed) => parsed,
        Err(message) => {
            return failure(
                input,
                display_text,
                class_symbol,
                creation_args,
                "object-spec.invalid-arg-type",
                message,
            );
        }
    };

    let value = match parse_number_value_arg(value_arg, representation, &kind) {
        Some(value) => value,
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
    };
    let port_type = match kind {
        NumberValueKind::Float => value_type_id_for_float_representation(representation),
        NumberValueKind::Int => value_type_id_for_int_representation(representation),
    }
    .expect("validated representation should map to a value type");
    let mut params = Map::new();
    params.insert("value".to_owned(), value);
    params.insert(
        "representation".to_owned(),
        Value::String(representation.to_owned()),
    );
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        candidate,
        params,
        stored_value_ports(port_type),
    )
}

fn parse_number_representation_arg<'a>(
    creation_args: &'a [ObjectSpecAtom],
    kind: &NumberValueKind,
) -> Result<(&'static str, Option<&'a ObjectSpecAtom>), String> {
    let default = match kind {
        NumberValueKind::Float => "f32",
        NumberValueKind::Int => "i32",
    };
    let Some(first) = creation_args.first() else {
        return Ok((default, None));
    };

    if let ObjectSpecAtom::Symbol(value) = first {
        if let Some(representation) = normalize_number_representation(value, kind) {
            return Ok((representation, creation_args.get(1)));
        }
        return Err(format!("{value} is not a valid numeric representation"));
    }

    if creation_args.len() > 1 {
        return Err("numeric representation must come before the value".to_owned());
    }
    Ok((default, Some(first)))
}

fn normalize_number_representation(
    representation: &str,
    kind: &NumberValueKind,
) -> Option<&'static str> {
    match kind {
        NumberValueKind::Float => match representation {
            "f64" => Some("f64"),
            "f32" => Some("f32"),
            "f16" => Some("f16"),
            "f8.e4m3" => Some("f8.e4m3"),
            "f8.e5m2" => Some("f8.e5m2"),
            "ufloat64" => Some("ufloat64"),
            "ufloat32" => Some("ufloat32"),
            "ufloat16" => Some("ufloat16"),
            "ufloat8" => Some("ufloat8"),
            _ => None,
        },
        NumberValueKind::Int => match representation {
            "i64" => Some("i64"),
            "i32" => Some("i32"),
            "i16" => Some("i16"),
            "i8" => Some("i8"),
            "u64" => Some("u64"),
            "u32" => Some("u32"),
            "u16" => Some("u16"),
            "u8" => Some("u8"),
            _ => None,
        },
    }
}

fn parse_number_value_arg(
    value_arg: Option<&ObjectSpecAtom>,
    representation: &str,
    kind: &NumberValueKind,
) -> Option<Value> {
    match kind {
        NumberValueKind::Float => value_arg
            .map(numeric_value)
            .unwrap_or(Some(0.0))
            .map(|value| json!(value)),
        NumberValueKind::Int if representation.starts_with('u') => value_arg
            .map(unsigned_json_value)
            .unwrap_or_else(|| Some(json!(0))),
        NumberValueKind::Int => value_arg
            .map(integer_value)
            .unwrap_or(Some(0))
            .map(|value| json!(value)),
    }
}

fn unsigned_json_value(atom: &ObjectSpecAtom) -> Option<Value> {
    match atom {
        ObjectSpecAtom::Int(value) if *value >= 0 => Some(json!(*value as u64)),
        ObjectSpecAtom::Symbol(value) => value.parse::<u64>().ok().map(|value| json!(value)),
        ObjectSpecAtom::Float(_) | ObjectSpecAtom::Bool(_) | ObjectSpecAtom::Int(_) => None,
    }
}
