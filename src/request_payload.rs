use serde_json::{Value, json};

use crate::{
    CURRENT_SCHEMA_VERSION, ProjectDocumentCurrent, ProjectRequestCurrent,
    RunProjectRequestCurrent, RuntimeIssue, RuntimeSession, RuntimeSessionLoadModeCurrent,
    RuntimeSessionLoadRequestCurrent, RuntimeSessionSnapshot,
    project_current::repair_project_load_edges_current, project_document_payload_schema_issues,
    project_document_validation_issues_current, schema_version_issue,
};

const RUNTIME_SESSION_LOAD_REQUEST_SCHEMA: &str = "skenion.runtime.session-load-request";

pub(crate) enum ProjectPayload {
    Current(Box<ProjectRequestCurrent>),
}

pub(crate) enum RunProjectPayload {
    Current(Box<RunProjectRequestCurrent>),
}

pub(crate) enum RuntimeSessionLoadPayload {
    Current {
        request: Box<RuntimeSessionLoadRequestCurrent>,
        repair_issues: Vec<RuntimeIssue>,
    },
}

pub(crate) fn decode_runtime_session_load_request_payload(
    value: Value,
) -> Result<RuntimeSessionLoadPayload, Vec<RuntimeIssue>> {
    if is_project_document(&value) {
        return Err(vec![RuntimeIssue::structured_error(
            "runtime.session-load.raw-project-rejected",
            "Runtime session load requires a skenion.runtime.session-load-request envelope; raw ProjectDocument bodies are no longer accepted.",
            json!({
                "schema": "skenion.runtime.session-load-request",
                "schemaVersion": CURRENT_SCHEMA_VERSION,
                "replacement": {
                    "schema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                    "fields": ["project", "mode", "precondition"]
                }
            }),
        )]);
    }

    match runtime_session_load_request_schema_version(&value).as_deref() {
        Some(CURRENT_SCHEMA_VERSION) => {
            decode_runtime_session_load_request_current(value).map(|(request, repair_issues)| {
                RuntimeSessionLoadPayload::Current {
                    request: Box::new(request),
                    repair_issues,
                }
            })
        }
        received => Err(vec![runtime_session_load_schema_version_issue(
            &value, received,
        )]),
    }
}

pub(crate) fn validate_session_load_precondition(
    session: &RuntimeSession,
    request: &RuntimeSessionLoadRequestCurrent,
) -> Result<(), Vec<RuntimeIssue>> {
    let snapshot = session.snapshot();
    match &request.mode {
        RuntimeSessionLoadModeCurrent::ForceReplace => Ok(()),
        RuntimeSessionLoadModeCurrent::LoadIfEmpty if !snapshot.loaded() => Ok(()),
        RuntimeSessionLoadModeCurrent::LoadIfEmpty => Err(vec![session_load_conflict_issue(
            request,
            &snapshot,
            "loadIfEmpty requires an empty Runtime session",
            Vec::new(),
        )]),
        RuntimeSessionLoadModeCurrent::ReplaceIfMatch => {
            let Some(current_project) = snapshot.project.as_ref() else {
                return Err(vec![session_load_conflict_issue(
                    request,
                    &snapshot,
                    "replaceIfMatch requires an existing Runtime session project",
                    Vec::new(),
                )]);
            };
            let Some(precondition) = request.precondition.as_ref() else {
                return Err(vec![RuntimeIssue::structured_error(
                    "runtime.session-load.precondition-required",
                    "replaceIfMatch requires a precondition",
                    session_load_request_details(request, &snapshot, Vec::new()),
                )]);
            };

            let mut mismatches = Vec::new();
            if let Some(expected) = &precondition.document_id
                && expected != &current_project.document_id
            {
                mismatches.push(json!({
                    "field": "documentId",
                    "expected": expected,
                    "actual": current_project.document_id,
                }));
            }
            if let Some(expected) = &precondition.session_revision {
                let actual = snapshot.session_revision.to_string();
                if expected != &actual {
                    mismatches.push(json!({
                        "field": "sessionRevision",
                        "expected": expected,
                        "actual": actual,
                    }));
                }
            }
            if let Some(expected) = &precondition.graph_revision {
                let actual = current_project.graph.revision.as_str();
                if expected != actual {
                    mismatches.push(json!({
                        "field": "graphRevision",
                        "expected": expected,
                        "actual": actual,
                    }));
                }
            }

            if mismatches.is_empty() {
                Ok(())
            } else {
                Err(vec![session_load_conflict_issue(
                    request,
                    &snapshot,
                    "replaceIfMatch precondition does not match the current Runtime session",
                    mismatches,
                )])
            }
        }
    }
}

pub(crate) fn decode_project_payload(value: Value) -> Result<ProjectPayload, Vec<RuntimeIssue>> {
    match project_schema_version(&value).as_deref() {
        Some(CURRENT_SCHEMA_VERSION) => decode_project_payload_current(value)
            .map(Box::new)
            .map(ProjectPayload::Current),
        received => Err(vec![
            schema_version_issue(project_schema_surface(&value), received)
                .expect("current schema version should have decoded as current 0.1"),
        ]),
    }
}

pub(crate) fn decode_run_project_payload(
    value: Value,
) -> Result<RunProjectPayload, Vec<RuntimeIssue>> {
    match project_schema_version(&value).as_deref() {
        Some(CURRENT_SCHEMA_VERSION) => decode_run_project_payload_current(value)
            .map(Box::new)
            .map(RunProjectPayload::Current),
        received => Err(vec![
            schema_version_issue(project_schema_surface(&value), received)
                .expect("current schema version should have decoded as current 0.1"),
        ]),
    }
}

fn decode_runtime_session_load_request_current(
    mut value: Value,
) -> Result<(RuntimeSessionLoadRequestCurrent, Vec<RuntimeIssue>), Vec<RuntimeIssue>> {
    let mut repair_issues = if let Some(project) = value.get_mut("project") {
        drop_obsolete_object_implementation_versions_current(project)
    } else {
        Vec::new()
    };

    if let Some(project) = value.get("project") {
        reject_top_level_nodes_current(project)?;
        let schema_issues = project_document_payload_schema_issues(project);
        if !schema_issues.is_empty() {
            return Err(schema_issues);
        }
    }
    let mut request = serde_json::from_value::<RuntimeSessionLoadRequestCurrent>(value)
        .map_err(invalid_runtime_session_load_payload)?;
    repair_issues.extend(repair_project_load_edges_current(&mut request.project));
    if let Err(report) = skenion_contracts::validate_runtime_session_load_request_v01(&request) {
        return Err(runtime_session_load_validation_issues_current(
            &request, &report,
        ));
    }
    Ok((request, repair_issues))
}

fn drop_obsolete_object_implementation_versions_current(value: &mut Value) -> Vec<RuntimeIssue> {
    let mut dropped = Vec::new();
    drop_obsolete_object_implementation_versions_at_current(value, "$", &mut dropped);
    dropped
        .into_iter()
        .map(|drop| {
            RuntimeIssue::structured_warning(
                "project.load.obsolete-field-dropped",
                format!(
                    "Runtime dropped obsolete object implementation version at {}.",
                    drop.path
                ),
                json!({
                    "surface": "project-load",
                    "field": "implementation.version",
                    "path": drop.path,
                    "objectId": drop.object_id,
                }),
            )
        })
        .collect()
}

#[derive(Debug)]
struct ObsoleteImplementationVersionDrop {
    path: String,
    object_id: Option<String>,
}

fn drop_obsolete_object_implementation_versions_at_current(
    value: &mut Value,
    path: &str,
    dropped: &mut Vec<ObsoleteImplementationVersionDrop>,
) {
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                drop_obsolete_object_implementation_versions_at_current(
                    item,
                    &format!("{path}[{index}]"),
                    dropped,
                );
            }
        }
        Value::Object(object) => {
            if is_object_implementation_ref_value_current(object)
                && object.remove("version").is_some()
            {
                dropped.push(ObsoleteImplementationVersionDrop {
                    path: format!("{path}.version"),
                    object_id: object
                        .get("objectId")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                });
            }

            for (key, child) in object.iter_mut() {
                drop_obsolete_object_implementation_versions_at_current(
                    child,
                    &format!("{path}.{}", load_payload_path_member_current(key)),
                    dropped,
                );
            }
        }
        _ => {}
    }
}

fn is_object_implementation_ref_value_current(object: &serde_json::Map<String, Value>) -> bool {
    object.get("provider").is_some() && object.get("objectId").and_then(Value::as_str).is_some()
}

fn load_payload_path_member_current(member: &str) -> String {
    if member
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        member.to_owned()
    } else {
        serde_json::to_string(member).expect("member name should serialize")
    }
}

fn runtime_session_load_request_schema_version(value: &Value) -> Option<String> {
    if value.get("schema").and_then(|schema| schema.as_str())
        != Some(RUNTIME_SESSION_LOAD_REQUEST_SCHEMA)
    {
        return None;
    }

    value
        .get("schemaVersion")
        .and_then(|version| version.as_str())
        .map(str::to_owned)
}

fn runtime_session_load_schema_version_issue(
    value: &Value,
    received_schema_version: Option<&str>,
) -> RuntimeIssue {
    let received_schema = value.get("schema").and_then(|schema| schema.as_str());
    if received_schema != Some(RUNTIME_SESSION_LOAD_REQUEST_SCHEMA) {
        return RuntimeIssue::structured_error(
            "runtime.session-load.invalid-schema",
            "Runtime session load requires a skenion.runtime.session-load-request envelope",
            json!({
                "expectedSchema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                "receivedSchema": received_schema,
                "expectedSchemaVersion": CURRENT_SCHEMA_VERSION,
                "receivedSchemaVersion": received_schema_version,
            }),
        );
    }

    match received_schema_version {
        Some(version) => RuntimeIssue::structured_error(
            "runtime.session-load.unsupported-schema-version",
            format!("unsupported Runtime session load schemaVersion: {version}"),
            json!({
                "expectedSchema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                "expectedSchemaVersion": CURRENT_SCHEMA_VERSION,
                "receivedSchemaVersion": version,
            }),
        ),
        None => RuntimeIssue::structured_error(
            "runtime.session-load.missing-schema-version",
            "missing schemaVersion in Runtime session load request",
            json!({
                "expectedSchema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                "expectedSchemaVersion": CURRENT_SCHEMA_VERSION,
                "receivedSchemaVersion": Value::Null,
            }),
        ),
    }
}

fn runtime_session_load_validation_issues_current(
    request: &RuntimeSessionLoadRequestCurrent,
    report: &skenion_contracts::ValidationReportV01,
) -> Vec<RuntimeIssue> {
    report
        .errors()
        .iter()
        .map(|error| {
            RuntimeIssue::structured_error(
                "runtime.session-load.invalid-0.1",
                error.message.clone(),
                json!({
                    "schema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                    "schemaVersion": request.schema_version,
                    "mode": runtime_session_load_mode_label(&request.mode),
                    "projectId": request.project.id,
                    "documentId": request.project.document_id,
                }),
            )
        })
        .collect()
}

fn invalid_runtime_session_load_payload(error: serde_json::Error) -> Vec<RuntimeIssue> {
    vec![RuntimeIssue::structured_error(
        "runtime.session-load.invalid-payload",
        format!("invalid Runtime session load request: {error}"),
        json!({
            "schema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
            "schemaVersion": CURRENT_SCHEMA_VERSION,
        }),
    )]
}

fn session_load_conflict_issue(
    request: &RuntimeSessionLoadRequestCurrent,
    snapshot: &RuntimeSessionSnapshot,
    message: &'static str,
    mismatches: Vec<Value>,
) -> RuntimeIssue {
    RuntimeIssue::structured_error(
        "runtime.session-load.conflict",
        message,
        session_load_request_details(request, snapshot, mismatches),
    )
}

fn session_load_request_details(
    request: &RuntimeSessionLoadRequestCurrent,
    snapshot: &RuntimeSessionSnapshot,
    mismatches: Vec<Value>,
) -> Value {
    let current = snapshot.project.as_ref().map(|project| {
        json!({
            "documentId": project.document_id,
            "projectId": project.id,
            "projectRevision": project.revision,
            "graphId": project.graph.id,
            "graphRevision": project.graph.revision,
            "sessionRevision": snapshot.session_revision.to_string(),
        })
    });

    json!({
        "requested": {
            "mode": runtime_session_load_mode_label(&request.mode),
            "documentId": request.project.document_id,
            "projectId": request.project.id,
            "projectRevision": request.project.revision,
            "graphId": request.project.graph.id,
            "graphRevision": request.project.graph.revision,
            "precondition": request.precondition,
        },
        "current": current,
        "mismatches": mismatches,
    })
}

fn runtime_session_load_mode_label(mode: &RuntimeSessionLoadModeCurrent) -> &'static str {
    match mode {
        RuntimeSessionLoadModeCurrent::LoadIfEmpty => "loadIfEmpty",
        RuntimeSessionLoadModeCurrent::ReplaceIfMatch => "replaceIfMatch",
        RuntimeSessionLoadModeCurrent::ForceReplace => "forceReplace",
    }
}

fn decode_project_payload_current(
    value: Value,
) -> Result<ProjectRequestCurrent, Vec<RuntimeIssue>> {
    if is_project_document_current(&value) {
        return decode_project_document_request_current(value);
    }

    serde_json::from_value(value).map_err(invalid_project_payload)
}

fn decode_run_project_payload_current(
    value: Value,
) -> Result<RunProjectRequestCurrent, Vec<RuntimeIssue>> {
    if is_project_document_current(&value) {
        return decode_run_project_document_request_current(value);
    }

    serde_json::from_value(value).map_err(invalid_project_payload)
}

fn decode_project_document_request_current(
    mut value: Value,
) -> Result<ProjectRequestCurrent, Vec<RuntimeIssue>> {
    reject_top_level_nodes_current(&value)?;
    let _ = take_frames_current(&mut value)?;
    let document = decode_project_document_current(value)?;
    Ok(ProjectRequestCurrent::from_project_document(
        document,
        Vec::new(),
    ))
}

fn decode_run_project_document_request_current(
    mut value: Value,
) -> Result<RunProjectRequestCurrent, Vec<RuntimeIssue>> {
    reject_top_level_nodes_current(&value)?;
    let frames = take_frames_current(&mut value)?;
    let document = decode_project_document_current(value)?;
    Ok(RunProjectRequestCurrent::from_project_document(
        document,
        Vec::new(),
        frames,
    ))
}

fn decode_project_document_current(
    value: Value,
) -> Result<ProjectDocumentCurrent, Vec<RuntimeIssue>> {
    let schema_issues = project_document_payload_schema_issues(&value);
    if !schema_issues.is_empty() {
        return Err(schema_issues);
    }
    let document =
        serde_json::from_value::<ProjectDocumentCurrent>(value).map_err(invalid_project_payload)?;
    if let Err(report) = skenion_contracts::validate_project_document_v01(&document) {
        return Err(project_document_validation_issues_current(
            &document, &report,
        ));
    }
    Ok(document)
}

fn reject_top_level_nodes_current(value: &Value) -> Result<(), Vec<RuntimeIssue>> {
    if value.get("nodes").is_none() {
        return Ok(());
    }

    Err(vec![RuntimeIssue::structured_error(
        "project.document.top-level-nodes-rejected",
        "ProjectDocument payloads must not include top-level nodes; node definitions must come from Runtime registry/catalog sources or an explicit legacy ProjectRequest wrapper",
        json!({
            "surface": "project",
            "field": "nodes",
            "schema": "skenion.project",
        }),
    )])
}

fn take_frames_current(value: &mut Value) -> Result<Option<usize>, Vec<RuntimeIssue>> {
    let frames = value
        .as_object_mut()
        .and_then(|object| object.remove("frames"))
        .unwrap_or(Value::Null);
    serde_json::from_value(frames).map_err(invalid_project_payload)
}

fn project_schema_version(value: &Value) -> Option<String> {
    if is_project_document(value) {
        return value
            .get("schemaVersion")
            .and_then(|version| version.as_str())
            .map(str::to_owned);
    }

    value
        .get("graph")
        .and_then(|graph| graph.get("schemaVersion"))
        .and_then(|version| version.as_str())
        .map(str::to_owned)
}

fn is_project_document_current(value: &Value) -> bool {
    is_project_document(value)
        && value
            .get("schemaVersion")
            .and_then(|version| version.as_str())
            == Some(CURRENT_SCHEMA_VERSION)
}

fn is_project_document(value: &Value) -> bool {
    value.get("schema").and_then(|schema| schema.as_str()) == Some("skenion.project")
}

fn project_schema_surface(value: &Value) -> &'static str {
    if is_project_document(value) {
        "project"
    } else {
        "graph"
    }
}

fn invalid_project_payload(error: serde_json::Error) -> Vec<RuntimeIssue> {
    vec![RuntimeIssue::error(format!(
        "invalid project request: {error}"
    ))]
}
