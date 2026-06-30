use serde_json::{Map, Value};

use super::super::{ObjectRegistryCandidate, ObjectSpecAtom, ObjectSpecResolution};
use super::atoms::symbol_value;
use super::outcome::{failure, success};

pub(super) fn resolve_named_ref_object(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    param_key: &'static str,
    count_message: &'static str,
) -> ObjectSpecResolution {
    if creation_args.len() != 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-count",
            count_message,
        );
    }
    let Some(reference) = symbol_value(&creation_args[0]) else {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-type",
            format!("{class_symbol} reference argument must be a symbol"),
        );
    };
    let mut params = Map::new();
    params.insert(param_key.to_owned(), Value::String(reference));
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        candidate,
        params,
        Vec::new(),
    )
}

pub(super) fn resolve_optional_named_ref_object(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    param_key: &'static str,
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
    let mut params = Map::new();
    if let Some(arg) = creation_args.first() {
        let Some(reference) = symbol_value(arg) else {
            return failure(
                input,
                display_text,
                class_symbol,
                creation_args,
                "object-spec.invalid-arg-type",
                format!("{class_symbol} reference argument must be a symbol"),
            );
        };
        params.insert(param_key.to_owned(), Value::String(reference));
    }
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        candidate,
        params,
        Vec::new(),
    )
}
