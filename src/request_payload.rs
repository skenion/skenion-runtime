use serde_json::{Value, json};

use crate::{
    CURRENT_SCHEMA_VERSION, ProjectDocumentCurrent, ProjectRequestCurrent,
    RunProjectRequestCurrent, RuntimeDiagnostic, RuntimeSession, RuntimeSessionLoadModeCurrent,
    RuntimeSessionLoadRequestCurrent, RuntimeSessionSnapshot,
    project_document_payload_schema_diagnostics, project_document_validation_diagnostics_current,
    schema_version_diagnostic,
};

const RUNTIME_SESSION_LOAD_REQUEST_SCHEMA: &str = "skenion.runtime.session-load-request";

pub(crate) enum ProjectPayload {
    Current(Box<ProjectRequestCurrent>),
}

pub(crate) enum RunProjectPayload {
    Current(Box<RunProjectRequestCurrent>),
}

pub(crate) enum RuntimeSessionLoadPayload {
    Current(Box<RuntimeSessionLoadRequestCurrent>),
}

pub(crate) fn decode_runtime_session_load_request_payload(
    value: Value,
) -> Result<RuntimeSessionLoadPayload, Vec<RuntimeDiagnostic>> {
    if is_project_document(&value) {
        return Err(vec![RuntimeDiagnostic::structured_error(
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
        Some(CURRENT_SCHEMA_VERSION) => decode_runtime_session_load_request_current(value)
            .map(Box::new)
            .map(RuntimeSessionLoadPayload::Current),
        received => Err(vec![runtime_session_load_schema_version_diagnostic(
            &value, received,
        )]),
    }
}

pub(crate) fn validate_session_load_precondition(
    session: &RuntimeSession,
    request: &RuntimeSessionLoadRequestCurrent,
) -> Result<(), Vec<RuntimeDiagnostic>> {
    let snapshot = session.snapshot();
    match &request.mode {
        RuntimeSessionLoadModeCurrent::ForceReplace => Ok(()),
        RuntimeSessionLoadModeCurrent::LoadIfEmpty if !snapshot.loaded() => Ok(()),
        RuntimeSessionLoadModeCurrent::LoadIfEmpty => Err(vec![session_load_conflict_diagnostic(
            request,
            &snapshot,
            "loadIfEmpty requires an empty Runtime session",
            Vec::new(),
        )]),
        RuntimeSessionLoadModeCurrent::ReplaceIfMatch => {
            let Some(current_project) = snapshot.project.as_ref() else {
                return Err(vec![session_load_conflict_diagnostic(
                    request,
                    &snapshot,
                    "replaceIfMatch requires an existing Runtime session project",
                    Vec::new(),
                )]);
            };
            let Some(precondition) = request.precondition.as_ref() else {
                return Err(vec![RuntimeDiagnostic::structured_error(
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
                Err(vec![session_load_conflict_diagnostic(
                    request,
                    &snapshot,
                    "replaceIfMatch precondition does not match the current Runtime session",
                    mismatches,
                )])
            }
        }
    }
}

pub(crate) fn decode_project_payload(
    value: Value,
) -> Result<ProjectPayload, Vec<RuntimeDiagnostic>> {
    match project_schema_version(&value).as_deref() {
        Some(CURRENT_SCHEMA_VERSION) => decode_project_payload_current(value)
            .map(Box::new)
            .map(ProjectPayload::Current),
        received => Err(vec![
            schema_version_diagnostic(project_schema_surface(&value), received)
                .expect("current schema version should have decoded as current 0.1"),
        ]),
    }
}

pub(crate) fn decode_run_project_payload(
    value: Value,
) -> Result<RunProjectPayload, Vec<RuntimeDiagnostic>> {
    match project_schema_version(&value).as_deref() {
        Some(CURRENT_SCHEMA_VERSION) => decode_run_project_payload_current(value)
            .map(Box::new)
            .map(RunProjectPayload::Current),
        received => Err(vec![
            schema_version_diagnostic(project_schema_surface(&value), received)
                .expect("current schema version should have decoded as current 0.1"),
        ]),
    }
}

fn decode_runtime_session_load_request_current(
    value: Value,
) -> Result<RuntimeSessionLoadRequestCurrent, Vec<RuntimeDiagnostic>> {
    if let Some(project) = value.get("project") {
        reject_top_level_nodes_current(project)?;
        let schema_diagnostics = project_document_payload_schema_diagnostics(project);
        if !schema_diagnostics.is_empty() {
            return Err(schema_diagnostics);
        }
    }
    let request = serde_json::from_value::<RuntimeSessionLoadRequestCurrent>(value)
        .map_err(invalid_runtime_session_load_payload)?;
    if let Err(report) = skenion_contracts::validate_runtime_session_load_request_v01(&request) {
        return Err(runtime_session_load_validation_diagnostics_current(
            &request, &report,
        ));
    }
    Ok(request)
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

fn runtime_session_load_schema_version_diagnostic(
    value: &Value,
    received_schema_version: Option<&str>,
) -> RuntimeDiagnostic {
    let received_schema = value.get("schema").and_then(|schema| schema.as_str());
    if received_schema != Some(RUNTIME_SESSION_LOAD_REQUEST_SCHEMA) {
        return RuntimeDiagnostic::structured_error(
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
        Some(version) => RuntimeDiagnostic::structured_error(
            "runtime.session-load.unsupported-schema-version",
            format!("unsupported Runtime session load schemaVersion: {version}"),
            json!({
                "expectedSchema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                "expectedSchemaVersion": CURRENT_SCHEMA_VERSION,
                "receivedSchemaVersion": version,
            }),
        ),
        None => RuntimeDiagnostic::structured_error(
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

fn runtime_session_load_validation_diagnostics_current(
    request: &RuntimeSessionLoadRequestCurrent,
    report: &skenion_contracts::ValidationReportV01,
) -> Vec<RuntimeDiagnostic> {
    report
        .errors()
        .iter()
        .map(|error| {
            RuntimeDiagnostic::structured_error(
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

fn invalid_runtime_session_load_payload(error: serde_json::Error) -> Vec<RuntimeDiagnostic> {
    vec![RuntimeDiagnostic::structured_error(
        "runtime.session-load.invalid-payload",
        format!("invalid Runtime session load request: {error}"),
        json!({
            "schema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
            "schemaVersion": CURRENT_SCHEMA_VERSION,
        }),
    )]
}

fn session_load_conflict_diagnostic(
    request: &RuntimeSessionLoadRequestCurrent,
    snapshot: &RuntimeSessionSnapshot,
    message: &'static str,
    mismatches: Vec<Value>,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic::structured_error(
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
) -> Result<ProjectRequestCurrent, Vec<RuntimeDiagnostic>> {
    if is_project_document_current(&value) {
        return decode_project_document_request_current(value);
    }

    serde_json::from_value(value).map_err(invalid_project_payload)
}

fn decode_run_project_payload_current(
    value: Value,
) -> Result<RunProjectRequestCurrent, Vec<RuntimeDiagnostic>> {
    if is_project_document_current(&value) {
        return decode_run_project_document_request_current(value);
    }

    serde_json::from_value(value).map_err(invalid_project_payload)
}

fn decode_project_document_request_current(
    mut value: Value,
) -> Result<ProjectRequestCurrent, Vec<RuntimeDiagnostic>> {
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
) -> Result<RunProjectRequestCurrent, Vec<RuntimeDiagnostic>> {
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
) -> Result<ProjectDocumentCurrent, Vec<RuntimeDiagnostic>> {
    let schema_diagnostics = project_document_payload_schema_diagnostics(&value);
    if !schema_diagnostics.is_empty() {
        return Err(schema_diagnostics);
    }
    let document =
        serde_json::from_value::<ProjectDocumentCurrent>(value).map_err(invalid_project_payload)?;
    if let Err(report) = skenion_contracts::validate_project_document_v01(&document) {
        return Err(project_document_validation_diagnostics_current(
            &document, &report,
        ));
    }
    Ok(document)
}

fn reject_top_level_nodes_current(value: &Value) -> Result<(), Vec<RuntimeDiagnostic>> {
    if value.get("nodes").is_none() {
        return Ok(());
    }

    Err(vec![RuntimeDiagnostic::structured_error(
        "project.document.top-level-nodes-rejected",
        "ProjectDocument payloads must not include top-level nodes; node definitions must come from Runtime registry/catalog sources or an explicit legacy ProjectRequest wrapper",
        json!({
            "surface": "project",
            "field": "nodes",
            "schema": "skenion.project",
        }),
    )])
}

fn take_frames_current(value: &mut Value) -> Result<Option<usize>, Vec<RuntimeDiagnostic>> {
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

fn invalid_project_payload(error: serde_json::Error) -> Vec<RuntimeDiagnostic> {
    vec![RuntimeDiagnostic::error(format!(
        "invalid project request: {error}"
    ))]
}
