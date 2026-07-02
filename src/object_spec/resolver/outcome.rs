use serde_json::Map;
use skenion_contracts::{
    ObjectResolutionCandidateV01, ObjectResolutionIssueCodeV01, ObjectResolutionIssueV01,
    ObjectResolutionStatusV01, ObjectResolutionV01, PackageIssueSeverityV01,
};

use super::super::{
    ObjectRegistryCandidate, ObjectSpecAtom, ObjectSpecCandidateSummary, ObjectSpecIssue,
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
            issues: Vec::new(),
        },
        params,
        instance_ports,
        candidates: vec![summary],
        issues: Vec::new(),
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
    ObjectSpecResolution {
        input: input.to_owned(),
        display_text,
        class_symbol: class_symbol.to_owned(),
        creation_args,
        implementation: None,
        object_resolution: ObjectResolutionV01 {
            status: ObjectResolutionStatusV01::Unresolved,
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
            issues: vec![ObjectResolutionIssueV01 {
                severity: PackageIssueSeverityV01::Error,
                code: object_resolution_issue_code(&code),
                message: message.clone(),
                details: None,
            }],
        },
        params: Map::new(),
        instance_ports: Vec::new(),
        candidates,
        issues: vec![ObjectSpecIssue { code, message }],
    }
}

pub(super) fn failure_for_selected_candidate(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ObjectSpecResolution {
    let code = code.into();
    let message = message.into();
    let summary = candidate.summary();
    ObjectSpecResolution {
        input: input.to_owned(),
        display_text: display_text.clone(),
        class_symbol: class_symbol.to_owned(),
        creation_args,
        implementation: Some(candidate.implementation.clone()),
        object_resolution: ObjectResolutionV01 {
            status: ObjectResolutionStatusV01::Error,
            selected_spec: Some(display_text),
            candidates: vec![ObjectResolutionCandidateV01 {
                implementation: candidate.implementation.clone(),
                object_spec: candidate.canonical_object_spec(),
                display_name: Some(candidate.display_name.clone()),
                reason: Some("selected".to_owned()),
            }],
            issues: vec![ObjectResolutionIssueV01 {
                severity: PackageIssueSeverityV01::Error,
                code: object_resolution_issue_code(&code),
                message: message.clone(),
                details: None,
            }],
        },
        params: Map::new(),
        instance_ports: Vec::new(),
        candidates: vec![summary],
        issues: vec![ObjectSpecIssue { code, message }],
    }
}

pub(super) fn object_resolution_issue_code(code: &str) -> ObjectResolutionIssueCodeV01 {
    match code {
        "object-spec.ambiguous" => ObjectResolutionIssueCodeV01::ResolutionAmbiguous,
        "object-spec.provider-unavailable" => ObjectResolutionIssueCodeV01::ImplementationMissing,
        _ => ObjectResolutionIssueCodeV01::ResolutionUnresolved,
    }
}
