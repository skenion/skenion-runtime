use super::super::{ObjectRegistryCandidate, ObjectSpecResolution, ParsedObjectSpec};
use super::outcome::{failure_for_selected_candidate, failure_with_candidates, success};

pub(in crate::object_spec) fn construct_package_object(
    parsed: ParsedObjectSpec,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let ParsedObjectSpec {
        input,
        display_text,
        class_symbol,
        creation_args,
    } = parsed;
    if !creation_args.is_empty() {
        return failure_with_candidates(
            &input,
            display_text,
            &class_symbol,
            creation_args,
            vec![candidate.summary()],
            "object-spec.invalid-arg-count",
            format!("{class_symbol} package object shortcut accepts no creation arguments"),
        );
    }

    let Some(package) = candidate.package.as_ref() else {
        return failure_for_selected_candidate(
            &input,
            display_text,
            &class_symbol,
            creation_args,
            candidate,
            "object-spec.provider-unavailable",
            "package object candidate is missing package metadata",
        );
    };

    let Some(_root_path) = package.root_path.as_ref() else {
        return failure_for_selected_candidate(
            &input,
            display_text,
            &class_symbol,
            creation_args,
            candidate,
            "object-spec.provider-unavailable",
            format!(
                "package object {} from {} has no local package root",
                candidate.implementation.object_id, package.package_id
            ),
        );
    };

    let definition = match crate::object_spec::load_package_object_definition(candidate) {
        Some(definition) => definition,
        None => {
            return failure_for_selected_candidate(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                candidate,
                "object-spec.provider-unavailable",
                format!(
                    "package object definition {} is not readable",
                    package.definition_path
                ),
            );
        }
    };

    success(
        &input,
        display_text,
        &class_symbol,
        creation_args,
        candidate,
        serde_json::Map::new(),
        definition
            .ports
            .iter()
            .map(crate::object_spec::object_spec_port_from_current)
            .collect(),
    )
}
