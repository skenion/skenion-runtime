use serde_json::Map;
use skenion_contracts::{
    ObjectResolutionCandidateV01, ObjectResolutionDiagnosticCodeV01, ObjectResolutionDiagnosticV01,
    ObjectResolutionStatusV01, ObjectResolutionV01, PackageDiagnosticSeverityV01,
};

use super::super::{
    ObjectRegistryCandidate, ObjectSpecAtom, ObjectSpecCandidateSummary, ObjectSpecDiagnostic,
    ObjectSpecPort, ObjectSpecResolution,
};

pub(super) fn success(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    params: Map<String, serde_json::Value>,
    instance_ports: Vec<ObjectSpecPort>,
) -> ObjectSpecResolution {
    let selected_spec = display_text.clone();
    let summary = candidate.summary();
    ObjectSpecResolution {
        input: input.to_owned(),
        display_text,
        class_symbol: class_symbol.to_owned(),
        creation_args,
        implementation: Some(candidate.implementation.clone()),
        object_resolution: ObjectResolutionV01 {
            status: ObjectResolutionStatusV01::Resolved,
            selected_spec: Some(selected_spec),
            candidates: vec![ObjectResolutionCandidateV01 {
                implementation: candidate.implementation.clone(),
                object_spec: candidate.canonical_object_spec(),
                display_name: Some(candidate.display_name.clone()),
                reason: Some("selected".to_owned()),
            }],
            diagnostics: Vec::new(),
        },
        params,
        instance_ports,
        candidates: vec![summary],
        diagnostics: Vec::new(),
    }
}

pub(in crate::object_spec) fn failure(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ObjectSpecResolution {
    let code = code.into();
    failure_with_candidates(
        input,
        display_text,
        class_symbol,
        creation_args,
        Vec::new(),
        code,
        message,
    )
}

pub(super) fn failure_with_candidates(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidates: Vec<ObjectSpecCandidateSummary>,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ObjectSpecResolution {
    let code = code.into();
    let message = message.into();
    let status = resolution_status_for_code(&code);
    ObjectSpecResolution {
        input: input.to_owned(),
        display_text,
        class_symbol: class_symbol.to_owned(),
        creation_args,
        implementation: None,
        object_resolution: ObjectResolutionV01 {
            status,
            selected_spec: None,
            candidates: candidates
                .iter()
                .map(|candidate| ObjectResolutionCandidateV01 {
                    implementation: candidate.implementation.clone(),
                    object_spec: candidate.object_spec.clone(),
                    display_name: Some(candidate.display_name.clone()),
                    reason: None,
                })
                .collect(),
            diagnostics: vec![ObjectResolutionDiagnosticV01 {
                severity: PackageDiagnosticSeverityV01::Error,
                code: object_resolution_diagnostic_code(&code),
                message: message.clone(),
                details: None,
            }],
        },
        params: Map::new(),
        instance_ports: Vec::new(),
        candidates,
        diagnostics: vec![ObjectSpecDiagnostic { code, message }],
    }
}

fn resolution_status_for_code(code: &str) -> ObjectResolutionStatusV01 {
    match code {
        "object-spec.ambiguous" => ObjectResolutionStatusV01::Ambiguous,
        "object-spec.provider-unavailable" => ObjectResolutionStatusV01::Missing,
        _ => ObjectResolutionStatusV01::Unresolved,
    }
}

pub(super) fn object_resolution_diagnostic_code(code: &str) -> ObjectResolutionDiagnosticCodeV01 {
    match code {
        "object-spec.ambiguous" => ObjectResolutionDiagnosticCodeV01::ResolutionAmbiguous,
        "object-spec.provider-unavailable" => {
            ObjectResolutionDiagnosticCodeV01::ImplementationMissing
        }
        _ => ObjectResolutionDiagnosticCodeV01::ResolutionUnresolved,
    }
}
