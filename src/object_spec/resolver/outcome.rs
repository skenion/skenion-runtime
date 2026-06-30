use serde_json::Map;

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
    let summary = candidate.summary();
    ObjectSpecResolution {
        input: input.to_owned(),
        display_text,
        class_symbol: class_symbol.to_owned(),
        creation_args,
        resolved_kind: Some(candidate.kind.clone()),
        resolved_kind_version: Some(candidate.kind_version.clone()),
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
    ObjectSpecResolution {
        input: input.to_owned(),
        display_text,
        class_symbol: class_symbol.to_owned(),
        creation_args,
        resolved_kind: None,
        resolved_kind_version: None,
        params: Map::new(),
        instance_ports: Vec::new(),
        candidates,
        diagnostics: vec![ObjectSpecDiagnostic {
            code: code.into(),
            message: message.into(),
        }],
    }
}
