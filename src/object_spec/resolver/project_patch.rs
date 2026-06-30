use serde_json::{Map, Value};

use super::super::{ObjectRegistryCandidate, ObjectSpecResolution, ParsedObjectSpec};
use super::atoms::symbol_value;
use super::outcome::{failure_with_candidates, success};

pub(in crate::object_spec) fn construct_project_patch(
    parsed: ParsedObjectSpec,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let ParsedObjectSpec {
        input,
        display_text,
        class_symbol,
        creation_args,
    } = parsed;
    let Some(patch) = candidate.project_patch.as_ref() else {
        return failure_with_candidates(
            &input,
            display_text,
            &class_symbol,
            creation_args,
            vec![candidate.summary()],
            "object-spec.unresolved",
            "project patch candidate is missing patch metadata",
        );
    };

    if matches!(class_symbol.as_str(), "p" | "object.core.subpatch") {
        if creation_args.len() != 1 {
            return failure_with_candidates(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                vec![candidate.summary()],
                "object-spec.invalid-arg-count",
                "subpatch object spec requires exactly one patch reference",
            );
        }
        let Some(reference) = symbol_value(&creation_args[0]) else {
            return failure_with_candidates(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                vec![candidate.summary()],
                "object-spec.invalid-arg-type",
                format!("{class_symbol} reference argument must be a symbol"),
            );
        };
        if reference != patch.patch_id {
            return failure_with_candidates(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                vec![candidate.summary()],
                "object-spec.unresolved",
                format!("project patch {reference} is not available in the active project"),
            );
        }
    } else if !creation_args.is_empty() {
        return failure_with_candidates(
            &input,
            display_text,
            &class_symbol,
            creation_args,
            vec![candidate.summary()],
            "object-spec.invalid-arg-count",
            format!("{class_symbol} project patch shortcut accepts no creation arguments"),
        );
    }

    let mut params = Map::new();
    params.insert("patchRef".to_owned(), Value::String(patch.patch_id.clone()));
    params.insert(
        "patchRevision".to_owned(),
        Value::String(patch.revision.clone()),
    );
    success(
        &input,
        display_text,
        &class_symbol,
        creation_args,
        candidate,
        params,
        patch.ports.clone(),
    )
}

pub(in crate::object_spec) fn explicit_project_patch_ref(
    parsed: &ParsedObjectSpec,
) -> Option<String> {
    if parsed.creation_args.len() != 1 {
        return None;
    }
    symbol_value(&parsed.creation_args[0])
}
