use super::{ObjectRegistryCandidate, ObjectSpecResolution, ParsedObjectSpec};

mod atoms;
mod audio;
mod control;
mod outcome;
mod package;
mod parser;
mod project_patch;
mod reference;

use audio::resolve_audio_object;
pub(super) use audio::unsupported_first_party_audio_message;
use control::{resolve_control_operator, resolve_control_value};
pub(super) use outcome::failure;
use outcome::failure_with_candidates;
pub(super) use package::construct_package_object;
pub(super) use parser::parse_object_spec_input_v01;
pub(super) use project_patch::{construct_project_patch, explicit_project_patch_ref};
use reference::{resolve_named_ref_object, resolve_optional_named_ref_object};

#[cfg(test)]
pub(super) use atoms::contract_object_spec_atom_to_runtime;
#[cfg(test)]
pub(super) use parser::runtime_object_spec_issue_code;

pub(super) fn construct_first_party_core(
    parsed: ParsedObjectSpec,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    if let Some(core) = candidate.core {
        return core.resolve(parsed, candidate);
    }

    let ParsedObjectSpec {
        input,
        display_text,
        class_symbol,
        creation_args,
    } = parsed;

    failure_with_candidates(
        &input,
        display_text,
        &class_symbol,
        creation_args,
        vec![candidate.summary()],
        "object-spec.unresolved",
        format!(
            "{} is registered but has no Runtime constructor",
            candidate.implementation.object_id
        ),
    )
}

pub(crate) fn resolve_core_control_operator(
    parsed: ParsedObjectSpec,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let ParsedObjectSpec {
        input,
        display_text,
        class_symbol,
        creation_args,
    } = parsed;
    resolve_control_operator(
        &input,
        display_text,
        &class_symbol,
        creation_args,
        candidate,
    )
}

pub(crate) fn resolve_core_control_value(
    parsed: ParsedObjectSpec,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let ParsedObjectSpec {
        input,
        display_text,
        class_symbol,
        creation_args,
    } = parsed;
    resolve_control_value(
        &input,
        display_text,
        &class_symbol,
        creation_args,
        candidate,
    )
}

pub(crate) fn resolve_core_audio(
    parsed: ParsedObjectSpec,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let ParsedObjectSpec {
        input,
        display_text,
        class_symbol,
        creation_args,
    } = parsed;
    resolve_audio_object(
        &input,
        display_text,
        &class_symbol,
        creation_args,
        candidate,
    )
}

pub(crate) fn resolve_core_subpatch(
    parsed: ParsedObjectSpec,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let ParsedObjectSpec {
        input,
        display_text,
        class_symbol,
        creation_args,
    } = parsed;
    resolve_named_ref_object(
        &input,
        display_text,
        &class_symbol,
        creation_args,
        candidate,
        "patchRef",
        "subpatch object spec requires exactly one patch reference",
    )
}

pub(crate) fn resolve_core_boundary_port(
    parsed: ParsedObjectSpec,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let ParsedObjectSpec {
        input,
        display_text,
        class_symbol,
        creation_args,
    } = parsed;
    resolve_optional_named_ref_object(
        &input,
        display_text,
        &class_symbol,
        creation_args,
        candidate,
        "portId",
    )
}

pub(super) fn unresolved_resolution(parsed: ParsedObjectSpec) -> ObjectSpecResolution {
    failure(
        &parsed.input,
        parsed.display_text,
        &parsed.class_symbol,
        parsed.creation_args,
        "object-spec.unresolved",
        format!(
            "{} is not available in the local Runtime object registry",
            parsed.class_symbol
        ),
    )
}

pub(super) fn ambiguous_resolution(
    parsed: ParsedObjectSpec,
    candidates: Vec<ObjectRegistryCandidate>,
) -> ObjectSpecResolution {
    let summaries = candidates
        .iter()
        .map(ObjectRegistryCandidate::summary)
        .collect::<Vec<_>>();
    let candidate_list = summaries
        .iter()
        .map(|candidate| format!("{} ({})", candidate.id, candidate.source))
        .collect::<Vec<_>>()
        .join(", ");
    failure_with_candidates(
        &parsed.input,
        parsed.display_text,
        &parsed.class_symbol,
        parsed.creation_args,
        summaries,
        "object-spec.ambiguous",
        format!(
            "{} matches multiple Runtime object candidates: {candidate_list}",
            parsed.class_symbol
        ),
    )
}
