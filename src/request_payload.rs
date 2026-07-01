use serde_json::{Value, json};

use crate::{
    CURRENT_SCHEMA_VERSION, RuntimeIssue, RuntimeSession, RuntimeSessionLoadModeCurrent,
    RuntimeSessionLoadRequestCurrent, RuntimeSessionSnapshot,
    project_current::{repair_project_load_edges_current, schema_version_issue},
};

const RUNTIME_SESSION_LOAD_REQUEST_SCHEMA: &str = "skenion.runtime.session-load-request";

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
        validate_project_load_schema_versions_current(project)?;
    }
    canonicalize_runtime_session_load_request_current(&mut value);
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

fn validate_project_load_schema_versions_current(project: &Value) -> Result<(), Vec<RuntimeIssue>> {
    if let Some(issue) = schema_version_issue(
        "project",
        project.get("schemaVersion").and_then(Value::as_str),
    ) {
        return Err(vec![issue]);
    }
    let graph = project.get("graph").unwrap_or(&Value::Null);
    if let Some(issue) =
        schema_version_issue("graph", graph.get("schemaVersion").and_then(Value::as_str))
    {
        return Err(vec![issue]);
    }
    if let Some(view_state) = project.get("viewState")
        && let Some(issue) = schema_version_issue(
            "view-state",
            view_state.get("schemaVersion").and_then(Value::as_str),
        )
    {
        return Err(vec![issue]);
    }
    Ok(())
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

fn canonicalize_runtime_session_load_request_current(value: &mut Value) {
    retain_object_fields(
        value,
        &["schema", "schemaVersion", "project", "mode", "precondition"],
    );
    if let Some(project) = value.get_mut("project") {
        canonicalize_project_document_current(project);
    }
    if let Some(precondition) = value.get_mut("precondition") {
        retain_object_fields(
            precondition,
            &["documentId", "sessionRevision", "graphRevision"],
        );
    }
}

fn canonicalize_project_document_current(value: &mut Value) {
    retain_object_fields(
        value,
        &[
            "schema",
            "schemaVersion",
            "id",
            "documentId",
            "revision",
            "metadata",
            "graph",
            "viewState",
            "patchLibrary",
            "packageDependencies",
            "packageLock",
            "resourceLock",
            "objectBindings",
            "tutorial",
            "help",
        ],
    );
    if let Some(graph) = value.get_mut("graph") {
        canonicalize_graph_document_current(graph);
    }
    if let Some(view_state) = value.get_mut("viewState") {
        canonicalize_view_state_current(view_state);
    }
    if let Some(patch_library) = value.get_mut("patchLibrary") {
        canonicalize_array_items(patch_library, canonicalize_patch_definition_current);
    }
    if let Some(object_bindings) = value.get_mut("objectBindings") {
        canonicalize_array_items(object_bindings, canonicalize_project_object_binding_current);
    }
}

fn canonicalize_patch_definition_current(value: &mut Value) {
    retain_object_fields(value, &["id", "revision", "metadata", "graph", "viewState"]);
    if let Some(graph) = value.get_mut("graph") {
        canonicalize_graph_document_current(graph);
    }
    if let Some(view_state) = value.get_mut("viewState") {
        canonicalize_view_state_current(view_state);
    }
}

fn canonicalize_graph_document_current(value: &mut Value) {
    retain_object_fields(
        value,
        &[
            "schema",
            "schemaVersion",
            "id",
            "revision",
            "nodes",
            "edges",
            "cableStyles",
        ],
    );
    if let Some(nodes) = value.get_mut("nodes") {
        canonicalize_array_items(nodes, canonicalize_graph_node_current);
    }
    if let Some(edges) = value.get_mut("edges") {
        canonicalize_array_items(edges, canonicalize_edge_current);
    }
    if let Some(cable_styles) = value.get_mut("cableStyles")
        && let Value::Object(styles) = cable_styles
    {
        for style in styles.values_mut() {
            retain_object_fields(style, &["color", "pattern", "width", "marker"]);
        }
    }
}

fn canonicalize_graph_node_current(value: &mut Value) {
    retain_object_fields(
        value,
        &[
            "id",
            "implementation",
            "objectSpec",
            "objectResolution",
            "bindingRef",
            "params",
            "ports",
            "portGroups",
        ],
    );
    if let Some(implementation) = value.get_mut("implementation") {
        canonicalize_object_implementation_current(implementation);
    }
    if let Some(object_resolution) = value.get_mut("objectResolution") {
        canonicalize_object_resolution_current(object_resolution);
    }
    if let Some(ports) = value.get_mut("ports") {
        canonicalize_array_items(ports, canonicalize_port_current);
    }
    if let Some(port_groups) = value.get_mut("portGroups") {
        canonicalize_array_items(port_groups, canonicalize_port_group_current);
    }
}

fn canonicalize_object_implementation_current(value: &mut Value) {
    retain_object_fields(value, &["provider", "objectId", "interfaceDigest"]);
    if let Some(provider) = value.get_mut("provider") {
        canonicalize_object_provider_current(provider);
    }
}

fn canonicalize_object_provider_current(value: &mut Value) {
    let Some(kind) = value.get("kind").and_then(Value::as_str) else {
        return;
    };
    match kind {
        "projectPatch" => retain_object_fields(
            value,
            &[
                "kind",
                "patchId",
                "revision",
                "interfaceRevision",
                "interfaceDigest",
            ],
        ),
        "package" => retain_object_fields(value, &["kind", "packageId", "lockEntryId", "version"]),
        _ => retain_object_fields(value, &["kind"]),
    }
}

fn canonicalize_object_resolution_current(value: &mut Value) {
    retain_object_fields(value, &["status", "selectedSpec", "candidates", "issues"]);
    if let Some(candidates) = value.get_mut("candidates") {
        canonicalize_array_items(candidates, canonicalize_object_resolution_candidate_current);
    }
    if let Some(issues) = value.get_mut("issues") {
        canonicalize_array_items(issues, canonicalize_object_resolution_issue_current);
    }
}

fn canonicalize_object_resolution_candidate_current(value: &mut Value) {
    retain_object_fields(
        value,
        &["implementation", "objectSpec", "displayName", "reason"],
    );
    if let Some(implementation) = value.get_mut("implementation") {
        canonicalize_object_implementation_current(implementation);
    }
}

fn canonicalize_object_resolution_issue_current(value: &mut Value) {
    retain_object_fields(value, &["severity", "code", "message", "details"]);
}

fn canonicalize_project_object_binding_current(value: &mut Value) {
    retain_object_fields(
        value,
        &[
            "id",
            "objectSpec",
            "status",
            "implementation",
            "candidates",
            "issues",
        ],
    );
    if let Some(implementation) = value.get_mut("implementation") {
        canonicalize_object_implementation_current(implementation);
    }
    if let Some(candidates) = value.get_mut("candidates") {
        canonicalize_array_items(candidates, canonicalize_object_resolution_candidate_current);
    }
    if let Some(issues) = value.get_mut("issues") {
        canonicalize_array_items(issues, canonicalize_object_resolution_issue_current);
    }
}

fn canonicalize_port_current(value: &mut Value) {
    retain_object_fields(
        value,
        &[
            "id",
            "direction",
            "type",
            "label",
            "rate",
            "accepts",
            "minConnections",
            "maxConnections",
            "mergePolicy",
            "fanOutPolicy",
            "triggerMode",
            "messageKeys",
            "defaultValue",
            "latch",
            "required",
            "styleKey",
            "group",
            "description",
        ],
    );
    if let Some(message_keys) = value.get_mut("messageKeys") {
        retain_object_fields(
            message_keys,
            &["accepted", "silent", "trigger", "store", "emit"],
        );
    }
}

fn canonicalize_port_group_current(value: &mut Value) {
    retain_object_fields(
        value,
        &[
            "id",
            "direction",
            "type",
            "minPorts",
            "label",
            "rate",
            "maxPorts",
            "ordered",
            "portIdPattern",
            "createLabel",
            "defaultPortSpec",
        ],
    );
    if let Some(default_port_spec) = value.get_mut("defaultPortSpec") {
        canonicalize_port_current(default_port_spec);
    }
}

fn canonicalize_edge_current(value: &mut Value) {
    retain_object_fields(
        value,
        &[
            "id",
            "source",
            "target",
            "resolvedType",
            "order",
            "enabled",
            "adapter",
            "feedback",
            "styleOverride",
            "label",
            "description",
        ],
    );
    if let Some(source) = value.get_mut("source") {
        retain_object_fields(source, &["nodeId", "portId"]);
    }
    if let Some(target) = value.get_mut("target") {
        retain_object_fields(target, &["nodeId", "portId"]);
    }
    if let Some(feedback) = value.get_mut("feedback") {
        retain_object_fields(
            feedback,
            &[
                "enabled",
                "boundary",
                "initialValue",
                "recursionLimit",
                "maxEventsPerTick",
                "maxIterationsPerFrame",
                "bufferMode",
                "intentional",
                "label",
            ],
        );
    }
}

fn canonicalize_view_state_current(value: &mut Value) {
    retain_object_fields(value, &["schema", "schemaVersion", "canvas"]);
    if let Some(canvas) = value.get_mut("canvas") {
        retain_object_fields(canvas, &["nodes"]);
        if let Some(nodes) = canvas.get_mut("nodes")
            && let Value::Object(nodes) = nodes
        {
            for node_view in nodes.values_mut() {
                retain_object_fields(node_view, &["x", "y", "width", "height", "collapsed"]);
            }
        }
    }
}

fn canonicalize_array_items(value: &mut Value, canonicalize: fn(&mut Value)) {
    if let Value::Array(items) = value {
        for item in items {
            canonicalize(item);
        }
    }
}

fn retain_object_fields(value: &mut Value, allowed: &[&str]) {
    if let Value::Object(object) = value {
        object.retain(|key, _| allowed.contains(&key.as_str()));
    }
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

fn reject_top_level_nodes_current(value: &Value) -> Result<(), Vec<RuntimeIssue>> {
    if value.get("nodes").is_none() {
        return Ok(());
    }

    Err(vec![RuntimeIssue::structured_error(
        "project.document.top-level-nodes-rejected",
        "ProjectDocument payloads must not include top-level nodes; node definitions must come from Runtime registry/catalog sources.",
        json!({
            "surface": "project",
            "field": "nodes",
            "schema": "skenion.project",
        }),
    )])
}

fn is_project_document(value: &Value) -> bool {
    value.get("schema").and_then(|schema| schema.as_str()) == Some("skenion.project")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_load_request_drops_unknown_fields_across_project_shapes() {
        let mut value = json!({
          "schema": "skenion.runtime.session-load-request",
          "schemaVersion": "0.1.0",
          "mode": "forceReplace",
          "ignored": true,
          "precondition": {
            "documentId": "10000000-0000-0000-0000-000000000001",
            "ignored": true
          },
          "project": {
            "schema": "skenion.project",
            "schemaVersion": "0.1.0",
            "id": "project",
            "documentId": "10000000-0000-0000-0000-000000000001",
            "revision": "1",
            "ignored": true,
            "metadata": { "title": "Project", "custom": true },
            "graph": {
              "schema": "skenion.graph",
              "schemaVersion": "0.1.0",
              "id": "root",
              "revision": "1",
              "ignored": true,
              "cableStyles": {
                "default": {
                  "color": "#fff",
                  "ignored": true
                }
              },
              "nodes": [
                {
                  "id": "object_1",
                  "implementation": {
                    "provider": {
                      "kind": "package",
                      "packageId": "example/package",
                      "lockEntryId": "lock",
                      "version": "1.0.0",
                      "ignored": true
                    },
                    "objectId": "adder",
                    "interfaceDigest": {
                      "algorithm": "sha256",
                      "value": "abc"
                    },
                    "ignored": true
                  },
                  "objectSpec": "+ 1",
                  "objectResolution": {
                    "status": "resolved",
                    "selectedSpec": "+ 1",
                    "ignored": true,
                    "candidates": [
                      {
                        "implementation": {
                          "provider": {
                            "kind": "projectPatch",
                            "patchId": "voice",
                            "revision": "1",
                            "ignored": true
                          },
                          "objectId": "voice",
                          "ignored": true
                        },
                        "objectSpec": "p voice",
                        "displayName": "Voice",
                        "reason": "exact",
                        "ignored": true
                      }
                    ],
                    "issues": [
                      {
                        "severity": "warning",
                        "code": "interface-drift",
                        "message": "drift",
                        "details": { "free": true },
                        "ignored": true
                      }
                    ]
                  },
                  "params": { "free": true },
                  "ports": [
                    {
                      "id": "in",
                      "direction": "input",
                      "type": "value.core.message",
                      "messageKeys": {
                        "accepted": ["float"],
                        "ignored": true
                      },
                      "ignored": true
                    }
                  ],
                  "portGroups": [
                    {
                      "id": "args",
                      "direction": "input",
                      "type": "value.core.message",
                      "minPorts": 0,
                      "defaultPortSpec": {
                        "id": "arg",
                        "direction": "input",
                        "type": "value.core.message",
                        "ignored": true
                      },
                      "ignored": true
                    }
                  ],
                  "ignored": true
                }
              ],
              "edges": [
                {
                  "id": "edge_1",
                  "source": { "nodeId": "a", "portId": "out", "ignored": true },
                  "target": { "nodeId": "b", "portId": "in", "ignored": true },
                  "feedback": {
                    "enabled": true,
                    "boundary": "tick",
                    "ignored": true
                  },
                  "ignored": true
                }
              ]
            },
            "viewState": {
              "schema": "skenion.view-state",
              "schemaVersion": "0.1.0",
              "ignored": true,
              "canvas": {
                "ignored": true,
                "nodes": {
                  "object_1": {
                    "x": 1.0,
                    "y": 2.0,
                    "ignored": true
                  }
                }
              }
            },
            "patchLibrary": [
              {
                "id": "voice",
                "revision": "1",
                "ignored": true,
                "graph": {
                  "schema": "skenion.graph",
                  "schemaVersion": "0.1.0",
                  "id": "voice",
                  "revision": "1",
                  "nodes": [],
                  "edges": [],
                  "ignored": true
                },
                "viewState": {
                  "schema": "skenion.view-state",
                  "schemaVersion": "0.1.0",
                  "canvas": { "nodes": {} },
                  "ignored": true
                }
              }
            ],
            "objectBindings": [
              {
                "id": "binding_1",
                "objectSpec": "pkg.object",
                "status": "resolved",
                "implementation": {
                  "provider": { "kind": "core", "ignored": true },
                  "objectId": "float",
                  "ignored": true
                },
                "candidates": [],
                "issues": [],
                "ignored": true
              }
            ],
            "tutorial": { "free": true },
            "help": { "free": true }
          }
        });

        canonicalize_runtime_session_load_request_current(&mut value);

        assert!(value.get("ignored").is_none());
        assert!(value["precondition"].get("ignored").is_none());
        assert!(value["project"].get("ignored").is_none());
        assert!(value["project"]["graph"].get("ignored").is_none());
        assert!(
            value["project"]["graph"]["cableStyles"]["default"]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["nodes"][0]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["nodes"][0]["implementation"]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["nodes"][0]["implementation"]["provider"]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["nodes"][0]["objectResolution"]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["nodes"][0]["objectResolution"]["candidates"][0]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["nodes"][0]["objectResolution"]["issues"][0]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["nodes"][0]["ports"][0]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["nodes"][0]["ports"][0]["messageKeys"]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["nodes"][0]["portGroups"][0]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["nodes"][0]["portGroups"][0]["defaultPortSpec"]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["edges"][0]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["edges"][0]["source"]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["graph"]["edges"][0]["feedback"]
                .get("ignored")
                .is_none()
        );
        assert!(value["project"]["viewState"].get("ignored").is_none());
        assert!(
            value["project"]["viewState"]["canvas"]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["viewState"]["canvas"]["nodes"]["object_1"]
                .get("ignored")
                .is_none()
        );
        assert!(value["project"]["patchLibrary"][0].get("ignored").is_none());
        assert!(
            value["project"]["patchLibrary"][0]["graph"]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["patchLibrary"][0]["viewState"]
                .get("ignored")
                .is_none()
        );
        assert!(
            value["project"]["objectBindings"][0]
                .get("ignored")
                .is_none()
        );
        assert_eq!(value["project"]["metadata"]["custom"], true);
        assert_eq!(value["project"]["tutorial"]["free"], true);
        assert_eq!(value["project"]["help"]["free"], true);
        assert_eq!(
            value["project"]["graph"]["nodes"][0]["params"]["free"],
            true
        );
    }

    #[test]
    fn schema_version_preflight_preserves_structured_project_load_issues() {
        let missing_project = validate_project_load_schema_versions_current(&json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0"
          }
        }))
        .expect_err("missing project schemaVersion should fail");
        assert_eq!(
            missing_project[0].code.as_deref(),
            Some("project.missing-schema-version")
        );
        assert_eq!(
            missing_project[0].details.as_ref().unwrap()["surface"],
            "project"
        );

        let unsupported_graph = validate_project_load_schema_versions_current(&json!({
          "schema": "skenion.project",
          "schemaVersion": "0.1.0",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "9.9.9"
          }
        }))
        .expect_err("unsupported graph schemaVersion should fail");
        assert_eq!(
            unsupported_graph[0].code.as_deref(),
            Some("project.unsupported-schema-version")
        );
        assert_eq!(
            unsupported_graph[0].details.as_ref().unwrap()["surface"],
            "graph"
        );

        let unsupported_view = validate_project_load_schema_versions_current(&json!({
          "schema": "skenion.project",
          "schemaVersion": "0.1.0",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0"
          },
          "viewState": {
            "schema": "skenion.view-state",
            "schemaVersion": "9.9.9"
          }
        }))
        .expect_err("unsupported viewState schemaVersion should fail");
        assert_eq!(
            unsupported_view[0].code.as_deref(),
            Some("project.unsupported-schema-version")
        );
        assert_eq!(
            unsupported_view[0].details.as_ref().unwrap()["surface"],
            "view-state"
        );
    }

    #[test]
    fn runtime_session_load_issue_helpers_are_structured() {
        let invalid_schema = runtime_session_load_schema_version_issue(
            &json!({
              "schema": "skenion.project",
              "schemaVersion": "0.1.0"
            }),
            Some("0.1.0"),
        );
        assert_eq!(
            invalid_schema.code.as_deref(),
            Some("runtime.session-load.invalid-schema")
        );

        let parse_error = serde_json::from_value::<RuntimeSessionLoadRequestCurrent>(json!({
          "schema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
          "schemaVersion": "0.1.0",
          "mode": "loadIfEmpty"
        }))
        .expect_err("missing project should not parse");
        let invalid_payload = invalid_runtime_session_load_payload(parse_error);
        assert_eq!(
            invalid_payload[0].code.as_deref(),
            Some("runtime.session-load.invalid-payload")
        );

        let request = serde_json::from_value::<RuntimeSessionLoadRequestCurrent>(json!({
          "schema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
          "schemaVersion": "0.1.0",
          "mode": "loadIfEmpty",
          "project": {
            "schema": "skenion.project",
            "schemaVersion": "0.1.0",
            "id": "",
            "documentId": "not-a-uuid",
            "revision": "1",
            "graph": {
              "schema": "skenion.graph",
              "schemaVersion": "0.1.0",
              "id": "graph",
              "revision": "1",
              "nodes": [],
              "edges": []
            },
            "viewState": {
              "schema": "skenion.view-state",
              "schemaVersion": "0.1.0",
              "canvas": { "nodes": {} }
            },
            "patchLibrary": []
          }
        }))
        .expect("contract-shaped invalid request should parse");
        let report = skenion_contracts::validate_runtime_session_load_request_v01(&request)
            .expect_err("invalid request should fail contract validation");
        let validation_issues = runtime_session_load_validation_issues_current(&request, &report);
        assert_eq!(
            validation_issues[0].code.as_deref(),
            Some("runtime.session-load.invalid-0.1")
        );
        assert_eq!(
            validation_issues[0].details.as_ref().unwrap()["projectId"],
            ""
        );
    }
}
