use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::PathBuf,
};

use serde_json::{Value, json};

use crate::{
    ControlMessage, ControlValue, EdgeEndpointCurrent, EdgeSpecCurrent, GraphDocumentCurrent,
    GraphPatch, PasteGraphFragmentRequest, PortSpecCurrent, ProjectRequestCurrent,
    RuntimeCollaborationChange, RuntimeControlEmission, RuntimeControlEventRequest,
    RuntimeControlReadRequest, RuntimeControlReadTarget, RuntimeIssue, RuntimeOperationEnvelope,
    RuntimeOperationIssue, ViewState,
};

fn core_impl(object_id: &str) -> crate::ObjectImplementationRefCurrent {
    crate::ObjectImplementationRefCurrent {
        provider: crate::ObjectProviderRefCurrent::Core,
        object_id: object_id.to_owned(),
        interface_digest: None,
    }
}

fn conflicting_node_definition(
    definition: &crate::NodeDefinitionCurrent,
) -> crate::NodeDefinitionCurrent {
    let mut conflict = definition.clone();
    let port = conflict
        .ports
        .first_mut()
        .expect("test node definition should include at least one port");
    port.port_type = "value.core.bool".to_owned();
    conflict
}

fn current_core_node_json(
    id: &str,
    object_id: &str,
    object_spec: &str,
    params: Value,
    ports: Value,
) -> Value {
    json!({
      "id": id,
      "implementation": {
        "provider": { "kind": "core" },
        "objectId": object_id
      },
      "objectSpec": object_spec,
      "objectResolution": {
        "status": "resolved",
        "candidates": [],
        "issues": []
      },
      "params": params,
      "ports": ports
    })
}

fn current_provider_node_json(id: &str, object_id: &str, params: Value, ports: Value) -> Value {
    json!({
      "id": id,
      "implementation": {
        "provider": {
          "kind": "package",
          "packageId": "test/runtime-fixtures",
          "version": "0.1.0"
        },
        "objectId": object_id
      },
      "objectSpec": object_id,
      "objectResolution": {
        "status": "resolved",
        "candidates": [],
        "issues": []
      },
      "params": params,
      "ports": ports
    })
}

fn current_unresolved_node_json(id: &str, object_spec: &str) -> Value {
    json!({
      "id": id,
      "objectSpec": object_spec,
      "objectResolution": {
        "status": "unresolved",
        "candidates": [],
          "issues": [
          {
            "code": "resolution-unresolved",
            "severity": "error",
            "message": format!("{object_spec} is not available in the local runtime registry.")
          }
        ]
      },
      "params": {
        "objectSpec": object_spec,
        "issueMessage": format!("{object_spec} is not available in the local runtime registry."),
        "requestedKind": object_spec
      },
      "ports": []
    })
}

fn normalize_current_fixture_value(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize_current_fixture_value(item);
            }
        }
        Value::Object(object) => {
            if object.contains_key("kind")
                && object.contains_key("kindVersion")
                && object.contains_key("ports")
            {
                let id = object
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("node")
                    .to_owned();
                let kind = object
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let object_spec = object
                    .get("objectSpec")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let params = object.remove("params").unwrap_or_else(|| json!({}));
                let ports = object.remove("ports").unwrap_or_else(|| json!([]));
                *value = if let Some(object_id) = kind.strip_prefix("object.core.") {
                    current_core_node_json(
                        &id,
                        object_id,
                        object_spec.as_deref().unwrap_or(object_id),
                        params,
                        ports,
                    )
                } else {
                    current_provider_node_json(&id, &kind, params, ports)
                };
                return;
            }
            for child in object.values_mut() {
                normalize_current_fixture_value(child);
            }
        }
        _ => {}
    }
}

fn current_fixture<T>(mut value: Value, label: &str) -> T
where
    T: serde::de::DeserializeOwned,
{
    normalize_current_fixture_value(&mut value);
    serde_json::from_value(value).unwrap_or_else(|error| panic!("{label}: {error}"))
}

fn temp_package_dir(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!(
        "skenion-runtime-session-package-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn test_checksum() -> skenion_contracts::PackageChecksumV01 {
    skenion_contracts::PackageChecksumV01 {
        algorithm: skenion_contracts::PackageChecksumAlgorithmV01::Sha256,
        value: "0".repeat(64),
    }
}

fn package_registry_with_definition(
    package_dir: PathBuf,
    object_id: &str,
    primary_spec: &str,
) -> crate::PackageRegistryListResponseV01 {
    crate::PackageRegistryListResponseV01 {
        ok: true,
        packages: vec![crate::PackageRegistryEntryV01 {
            package_id: "example/package".to_owned(),
            version: "0.56.0".to_owned(),
            category: skenion_contracts::PackageCategoryV01::Mixed,
            source: skenion_contracts::PackageSourceV01::Workspace,
            root: skenion_contracts::PackageRootKindV01::Package,
            trust: skenion_contracts::PackageTrustV01::Trusted,
            contracts: skenion_contracts::PackageContractsRequirementV01 {
                version: skenion_contracts::CONTRACTS_PACKAGE_VERSION.to_owned(),
            },
            runtime_abi_range: None,
            targets: Vec::new(),
            manifest_path: crate::RUNTIME_PACKAGE_MANIFEST_FILE.to_owned(),
            root_path: Some(package_dir),
            manifest_checksum: test_checksum(),
            provides: skenion_contracts::PackageProvidesV01 {
                objects: vec![skenion_contracts::PackageObjectExportV01 {
                    object_id: object_id.to_owned(),
                    primary_object_spec: primary_spec.to_owned(),
                    aliases: Vec::new(),
                    definition_path: "nodes/package-node.json".to_owned(),
                    description: None,
                    help_id: None,
                }],
                ..Default::default()
            },
            issues: Vec::new(),
        }],
        issues: Vec::new(),
    }
}

use super::{
    HistoryEntry, RuntimeHistoryEntryKind, RuntimeMutationRequest, RuntimePatchResponse,
    RuntimeSession, RuntimeViewPatch, RuntimeViewPatchOperation, lower_fragment_view_patch,
    lower_port_for_execution, remap_edge, runtime_binding_format_revision,
    runtime_issue_to_operation_issue, runtime_value_format_label, value_format_for_port_type,
};

#[test]
fn invalid_registry_load_returns_issues_without_revision_change() {
    let mut session = RuntimeSession::default();
    let mut request = sample_project_current();
    request.nodes[0].schema_version = "9.9.9".to_owned();

    let response = session.load_project_current(request);

    assert!(!response.ok);
    assert!(!response.snapshot.loaded());
    assert_eq!(response.snapshot.session_revision, 0);
    assert!(!response.issues.is_empty());
}

#[test]
fn package_registry_definitions_are_available_during_current_load() {
    let package_dir = temp_package_dir("load");
    fs::create_dir_all(package_dir.join("nodes")).unwrap();
    fs::write(
        package_dir.join("nodes/package-node.json"),
        serde_json::to_vec(&json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "example.package.node",
          "version": "0.56.0",
          "displayName": "Package Node",
          "category": "Package",
          "ports": value_f32_ports_current_json(),
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
        .unwrap(),
    )
    .unwrap();
    let package_registry =
        package_registry_with_definition(package_dir, "example.package.node", "pkg.node");
    let request: ProjectRequestCurrent = serde_json::from_value(json!({
      "graph": {
        "schema": "skenion.graph",
        "schemaVersion": "0.1.0",
        "id": "package-load",
        "revision": "1",
        "nodes": [
          {
            "id": "pkg_1",
            "implementation": {
              "provider": {
                "kind": "package",
                "packageId": "example/package",
                "version": "0.56.0"
              },
              "objectId": "example.package.node"
            },
            "objectSpec": "pkg.node",
            "objectResolution": {
              "status": "resolved",
              "candidates": [],
              "issues": []
            },
            "params": {},
            "ports": value_f32_ports_current_json()
          }
        ],
        "edges": []
      },
      "nodes": [],
      "viewState": {
        "schema": "skenion.view-state",
        "schemaVersion": "0.1.0",
        "canvas": {
          "nodes": {
            "pkg_1": { "x": 96.0, "y": 96.0 }
          }
        }
      }
    }))
    .expect("package-backed current project should parse");

    let mut session = RuntimeSession::default();
    let response = session.load_project_current_with_package_registry(
        request,
        Some(7),
        Some(package_registry),
    );

    assert!(response.ok, "{:?}", response.issues);
    assert_eq!(response.snapshot.package_registry_revision, Some(7));
    assert!(session.nodes_current.iter().any(
        |definition| definition.id == "example.package.node" && definition.version == "0.56.0"
    ));
    assert!(
        session
            .node_catalog_snapshot()
            .entries
            .iter()
            .any(|entry| entry.object_id == "example.package.node"
                && entry.primary_object_spec == "pkg.node")
    );
}

#[test]
fn preview_control_is_absent_without_loaded_project() {
    let session = RuntimeSession::default();
    let preview_control = session.preview_control_state_snapshot();

    assert!(preview_control.is_none());
}

#[test]
fn session_snapshot_accessors_and_view_mutation_builder_preserve_metadata() {
    let mut session = RuntimeSession::default();
    let empty = session.snapshot();
    assert!(!empty.loaded());
    assert_eq!(empty.graph_id(), None);
    assert_eq!(empty.graph_revision(), None);
    assert!(empty.view_state().is_none());

    let response = session.load_project_current(sample_project_current());
    assert!(response.ok);
    let snapshot = response.snapshot;
    assert!(snapshot.loaded());
    assert_eq!(snapshot.graph_id(), Some("minimal-value"));
    assert_eq!(snapshot.graph_revision(), Some("1"));
    assert!(snapshot.view_state().is_some());

    let request = RuntimeMutationRequest::view_patch(RuntimeViewPatch {
        base_view_revision: snapshot.view_revision,
        ops: Vec::new(),
    })
    .with_client_id("client-a")
    .with_description("move canvas");

    assert!(request.graph_patch.is_none());
    assert!(request.view_patch.is_some());
    assert_eq!(request.client_id.as_deref(), Some("client-a"));
    assert_eq!(request.description.as_deref(), Some("move canvas"));
}

#[test]
fn session_snapshot_derives_endpoint_binding_value_formats() {
    let mut session = RuntimeSession::default();

    let response = session.load_project_current(binding_project_current());

    assert!(response.ok, "{:?}", response.issues);
    assert_eq!(response.snapshot.binding_formats.len(), 1);
    let binding = &response.snapshot.binding_formats[0];
    assert_eq!(binding.binding_id, "edge_value_target");
    assert_eq!(binding.binding_epoch, 1);
    assert_eq!(binding.format_revision, 1);
    assert_eq!(binding.format_digest.as_ref().map(String::len), Some(64));
    assert_eq!(binding.value_format.value_type_id, "value.core.float32");
    assert_eq!(binding.value_format.format.as_deref(), Some("f32"));
    assert_eq!(binding.source.as_ref().unwrap().node_id, "value_1");
    assert_eq!(binding.source.as_ref().unwrap().port_id, "value");
    assert_eq!(binding.target.as_ref().unwrap().node_id, "target_1");
    assert_eq!(binding.target.as_ref().unwrap().port_id, "cold");

    let snapshot_json =
        serde_json::to_value(&response.snapshot).expect("snapshot should serialize");
    assert!(snapshot_json.get("bindingFormats").is_some());
}

#[test]
fn control_event_fails_without_loaded_project() {
    let mut session = RuntimeSession::default();

    let response =
        session.apply_control_event(set_control_request("value_1", "in", f32_value(32.0)));

    assert!(!response.ok);
    assert!(response.emitted.is_empty());
    assert_eq!(session.snapshot().session_revision, 0);
    assert!(response.issues[0].message.contains("no project loaded"));
}

#[test]
fn control_set_bang_and_in_follow_typed_value_semantics() {
    let mut session = RuntimeSession::default();
    assert!(load_sample_project(&mut session).ok);

    let set = session.apply_control_event(set_control_request("value_1", "in", f32_value(32.0)));
    assert!(set.ok);
    assert!(set.changed);
    assert!(set.emitted.is_empty());
    assert_eq!(session.snapshot().session_revision, 1);
    assert_eq!(session.snapshot().control_revision, 1);
    assert_eq!(session.control_revision(), 1);
    assert_eq!(set.control_revision, Some(1));
    assert_eq!(
        session.control_state_response().values.get("value_1"),
        Some(&ControlValue::float(32.0))
    );

    let same_set =
        session.apply_control_event(set_control_request("value_1", "in", f32_value(32.0)));
    assert!(same_set.ok);
    assert!(!same_set.changed);
    assert_eq!(same_set.control_revision, Some(1));

    let bang = session.apply_control_event(bang_control_request("value_1", "in"));
    assert!(bang.ok);
    assert_eq!(bang.emitted.len(), 1);
    assert_eq!(bang.emitted[0].node_id, "value_1");
    assert_eq!(bang.emitted[0].port_id, "value");
    assert_eq!(
        emitted_value(&bang.emitted[0]),
        Some(ControlValue::float(32.0))
    );
    assert!(bang.changed);
    assert_eq!(session.snapshot().session_revision, 1);
    assert_eq!(session.snapshot().control_revision, 2);
    assert_eq!(bang.control_revision, Some(2));

    let input = session.apply_control_event(control_request("value_1", "in", f32_value(12.0)));
    assert!(input.ok);
    assert!(input.changed);
    assert_eq!(
        emitted_value(&input.emitted[0]),
        Some(ControlValue::float(12.0))
    );
    assert_eq!(
        session.control_state_response().values.get("value_1"),
        Some(&ControlValue::float(12.0))
    );
    assert_eq!(
        session.control_state_response().values.get("target_1"),
        Some(&ControlValue::float(12.0))
    );
    assert_eq!(session.snapshot().session_revision, 1);
    assert_eq!(session.snapshot().control_revision, 3);
    assert_eq!(session.control_revision(), 3);
    assert_eq!(input.control_revision, Some(3));
}

#[test]
fn control_object_send_name_updates_typed_channel_state() {
    let mut session = RuntimeSession::default();
    assert!(
        session
            .load_project_current(object_routing_project_current())
            .ok
    );

    let response = session.apply_control_event(control_request("value_1", "in", f32_value(1.5)));

    assert!(response.ok);
    assert_eq!(response.emitted[0].node_id, "value_1");
    assert_eq!(response.emitted[0].port_id, "value");
    assert_eq!(
        emitted_value(&response.emitted[0]),
        Some(ControlValue::float(1.5))
    );
    assert_eq!(
        session
            .control_state_response()
            .channels
            .get("value.core.float32:speed"),
        Some(&ControlMessage::from_value(ControlValue::float(1.5)))
    );
}

#[test]
fn control_read_addresses_params_ports_and_state() {
    let mut session = RuntimeSession::default();
    let mut project = sample_project_current();
    project.graph.nodes[0]
        .params
        .insert("value".to_owned(), json!(0.0));
    assert!(session.load_project_current(project).ok);
    assert!(
        session
            .apply_control_event(set_control_request("value_1", "in", f32_value(32.0)))
            .ok
    );

    let param = session.read_control(control_read(
        "value_1",
        RuntimeControlReadTarget::Param,
        "value",
    ));
    assert!(param.ok);
    assert_eq!(
        param.value.unwrap(),
        json!({ "type": "json", "value": 0.0 })
    );

    let port = session.read_control(control_read(
        "value_1",
        RuntimeControlReadTarget::Port,
        "value",
    ));
    assert!(port.ok);
    assert_eq!(port.value.unwrap()["value"]["id"], json!("value"));

    let state = session.read_control(control_read(
        "value_1",
        RuntimeControlReadTarget::State,
        "value",
    ));
    assert!(state.ok);
    assert_eq!(
        state.value.unwrap(),
        json!({ "type": "float", "representation": "f32", "value": 32.0 })
    );
}

#[test]
fn invalid_control_read_reports_issues() {
    let mut session = RuntimeSession::default();
    let missing_session = session.read_control(control_read(
        "value_1",
        RuntimeControlReadTarget::State,
        "value",
    ));
    assert!(!missing_session.ok);
    assert!(
        missing_session.issues[0]
            .message
            .contains("no project loaded")
    );

    assert!(load_sample_project(&mut session).ok);
    let missing_port = session.read_control(control_read(
        "value_1",
        RuntimeControlReadTarget::Port,
        "missing",
    ));
    assert!(!missing_port.ok);
    assert!(
        missing_port.issues[0]
            .message
            .contains("port missing does not exist")
    );

    let missing_node = session.read_control(control_read(
        "missing",
        RuntimeControlReadTarget::State,
        "value",
    ));
    assert!(!missing_node.ok);
    assert!(
        missing_node.issues[0]
            .message
            .contains("node missing does not exist")
    );

    let missing_param = session.read_control(control_read(
        "value_1",
        RuntimeControlReadTarget::Param,
        "missing",
    ));
    assert!(!missing_param.ok);
    assert!(
        missing_param.issues[0]
            .message
            .contains("param missing does not exist")
    );

    let missing_state_id = session.read_control(control_read(
        "value_1",
        RuntimeControlReadTarget::State,
        "other",
    ));
    assert!(!missing_state_id.ok);
    assert!(
        missing_state_id.issues[0]
            .message
            .contains("state other does not exist")
    );

    assert!(
        session
            .load_project_current(debug_sink_project_current())
            .ok
    );
    let missing_runtime_state = session.read_control(control_read(
        "debug_1",
        RuntimeControlReadTarget::State,
        "value",
    ));
    assert!(!missing_runtime_state.ok);
    assert!(
        missing_runtime_state.issues[0]
            .message
            .contains("has no runtime control state")
    );
}

#[test]
fn invalid_control_event_does_not_mutate_state_or_revision() {
    let mut session = RuntimeSession::default();
    assert!(load_sample_project(&mut session).ok);
    assert!(
        session
            .apply_control_event(set_control_request("value_1", "in", f32_value(32.0)))
            .ok
    );
    let before = session.snapshot();

    let response = session.apply_control_event(control_request(
        "value_1",
        "in",
        ControlValue::color([0.0, 0.0, 0.0, 1.0]),
    ));

    assert!(!response.ok);
    assert!(response.emitted.is_empty());
    assert_eq!(session.snapshot().session_revision, before.session_revision);
    assert_eq!(
        session.control_state_response().values.get("value_1"),
        Some(&ControlValue::float(32.0))
    );
}

#[test]
fn failed_control_propagation_does_not_mutate_state_or_revision() {
    let mut session = RuntimeSession::default();
    assert!(load_sample_project(&mut session).ok);
    session
        .project
        .as_mut()
        .expect("project should remain loaded")
        .graph
        .edges = vec![EdgeSpecCurrent {
        id: "invalid_edge".to_owned(),
        source: EdgeEndpointCurrent {
            node_id: "value_1".to_owned(),
            port_id: "value".to_owned(),
        },
        target: EdgeEndpointCurrent {
            node_id: "target_1".to_owned(),
            port_id: "missing".to_owned(),
        },
        resolved_type: None,
        order: None,
        enabled: None,
        adapter: None,
        feedback: None,
        style_override: None,
        label: None,
        description: None,
    }];
    let before = session.snapshot();

    let response = session.apply_control_event(control_request("value_1", "in", f32_value(9.0)));

    assert!(!response.ok);
    assert!(response.issues[0].message.contains("port missing"));
    assert_eq!(response.control_revision, Some(before.control_revision));
    assert_eq!(session.snapshot().control_revision, before.control_revision);
    assert_eq!(
        session.control_state_response().values.get("value_1"),
        Some(&ControlValue::float(0.0))
    );
    assert_eq!(
        session.control_state_response().values.get("target_1"),
        Some(&ControlValue::float(0.0))
    );
}

#[test]
fn graph_patch_rebuilds_control_state_from_graph_params() {
    let mut session = RuntimeSession::default();
    assert!(load_sample_project(&mut session).ok);
    assert!(
        session
            .apply_control_event(set_control_request("value_1", "in", f32_value(32.0)))
            .ok
    );

    let response = session.apply_patch(set_value_patch("1", 0.75));

    assert_graph_patch_rejected(&response);
    assert_eq!(
        session.control_state_response().values.get("value_1"),
        Some(&ControlValue::float(32.0))
    );
}

#[test]
fn preview_context_requires_loaded_project_and_plan() {
    let mut session = RuntimeSession::default();

    let missing_project = session.preview_context();
    assert!(
        missing_project
            .unwrap_err()
            .first()
            .unwrap()
            .message
            .contains("no project loaded")
    );

    load_sample_project(&mut session);
    let context = session.preview_context().expect("context should exist");
    assert_eq!(context.graph_id, "minimal-value");
    assert_eq!(context.graph_revision, "1");
    assert_eq!(context.session_revision, 1);
    assert_eq!(context.plan.graph_id, "minimal-value");
    assert_eq!(
        context.control_state.value_for_node("value_1"),
        Some(&ControlValue::float(0.0))
    );

    session.plan = None;
    let missing_plan = session.preview_context();
    assert!(
        missing_plan
            .unwrap_err()
            .first()
            .unwrap()
            .message
            .contains("no execution plan available")
    );
}

#[test]
fn graph_and_view_state_accessors_return_loaded_project_copies() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);

    assert!(loaded.ok);
    assert_eq!(session.graph().unwrap().id, "minimal-value");
    assert!(
        session
            .view_state()
            .unwrap()
            .canvas
            .nodes
            .contains_key("value_1")
    );
}

#[test]
fn patch_without_loaded_session_returns_error() {
    let mut session = RuntimeSession::default();

    let response = session.apply_patch(set_value_patch("1", 0.75));

    assert!(!response.ok);
    assert!(!response.applied);
    assert!(!response.conflict);
    assert!(response.snapshot.project.is_none());
    assert!(!response.snapshot.loaded());
    assert!(
        response.issues[0]
            .message
            .contains("no project loaded in runtime session")
    );
}

#[test]
fn patch_with_matching_revision_applies_and_rebuilds_plan() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);
    assert!(loaded.ok);

    let response = session.apply_patch(set_value_patch("1", 0.75));

    assert_graph_patch_rejected(&response);
    assert_eq!(response.history.entries.len(), 0);
    assert_eq!(response.history.undo_depth, 0);
    assert_eq!(response.history.redo_depth, 0);
    assert_eq!(patch_graph(&response).revision, "1");
    assert_eq!(response.snapshot.graph_revision(), Some("1"));
    assert_eq!(response.snapshot.session_revision, 1);
    assert_eq!(
        session.control_state.value_for_node("value_1"),
        Some(&ControlValue::float(0.0))
    );
}

#[test]
fn unresolved_object_loads_session_with_error_issue() {
    let mut session = RuntimeSession::default();

    let response = session.load_project_current(unresolved_project_current());

    assert!(response.ok);
    assert!(response.snapshot.loaded());
    assert!(
        response
            .issues
            .iter()
            .any(|issue| { issue.message.contains("unresolved object user.manipulator") })
    );
    assert_eq!(session.snapshot().issues, response.issues);

    assert!(
        response
            .snapshot
            .issues
            .iter()
            .any(|issue| { issue.message.contains("unresolved object user.manipulator") })
    );
}

#[test]
fn repairable_invalid_edge_loads_after_dropping_edge() {
    let mut session = RuntimeSession::default();
    let mut project = sample_project_current();
    project.graph.edges[0].target.port_id = "value".to_owned();

    let response = session.load_project_current(project);

    assert!(response.ok);
    assert!(response.snapshot.loaded());
    assert_eq!(response.snapshot.graph_revision(), Some("2"));
    let loaded_project = response
        .snapshot
        .project
        .as_ref()
        .expect("project should load after repair");
    assert!(loaded_project.graph.edges.is_empty());
    assert_eq!(loaded_project.revision, "2");
    assert!(response.issues.iter().any(|issue| issue.code.as_deref()
        == Some("project.load.edge-dropped")
        && issue.severity == crate::IssueSeverity::Warning));
    assert_eq!(session.snapshot().issues, response.issues);
}

#[test]
fn replace_node_with_unresolved_object_applies_with_error_issue() {
    let mut session = RuntimeSession::default();
    let loaded = session.load_project_current(sample_project_current());
    assert!(loaded.ok);

    let target = skenion_contracts::GraphTargetRef {
        path: skenion_contracts::PatchPath::Root,
        base_revision: "1".to_owned(),
        target_revision: None,
    };
    let unresolved_node = current_fixture(
        unresolved_node_json("target_1", "user.manipulator"),
        "unresolved replacement node should parse",
    );
    let (response, dropped_edge_ids) =
        session.apply_object_node_replace_current(super::ApplyObjectNodeReplaceCurrentRequest {
            target,
            node: unresolved_node,
            view: None,
            definition: None,
            interface_incident_edge_policy: Some(
                skenion_contracts::InterfaceIncidentEdgePolicyV01::Drop,
            ),
            mutation: empty_runtime_mutation(),
        });

    assert!(response.ok);
    assert!(response.applied);
    assert_eq!(dropped_edge_ids, vec!["edge_value_target".to_owned()]);
    assert!(response.snapshot.loaded());
    assert_eq!(patch_graph(&response).revision, "2");
    let replaced = patch_graph(&response)
        .nodes
        .iter()
        .find(|node| node.id == "target_1")
        .expect("target node should remain present");
    assert!(replaced.implementation.is_none());
    assert_eq!(replaced.object_spec.as_deref(), Some("user.manipulator"));
}

#[test]
fn patch_with_wrong_base_revision_conflicts_without_mutating_session() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);

    let response = session.apply_patch(set_value_patch("0", 0.75));
    let snapshot = session.snapshot();

    assert_graph_patch_rejected(&response);
    assert!(latest_history_entry(&response).is_none());
    assert!((response.history.entries).is_empty());
    assert_eq!(patch_graph(&response).revision, "1");
    assert_eq!(snapshot.graph_revision(), Some("1"));
    assert_eq!(snapshot.session_revision, 1);
}

#[test]
fn invalid_patch_operations_do_not_mutate_session() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);

    let duplicate = session.apply_patch(duplicate_edge_patch());
    let missing = session.apply_patch(missing_node_patch());
    let snapshot = session.snapshot();

    assert_graph_patch_rejected(&duplicate);
    assert!(latest_history_entry(&duplicate).is_none());
    assert!((duplicate.history.entries).is_empty());
    assert_graph_patch_rejected(&missing);
    assert_eq!(snapshot.graph_revision(), Some("1"));
    assert_eq!(snapshot.session_revision, 1);
}

#[test]
fn incompatible_patch_result_does_not_mutate_session() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);

    let response = session.apply_patch(incompatible_edge_patch());
    let snapshot = session.snapshot();

    assert_graph_patch_rejected(&response);
    assert_eq!(snapshot.graph_revision(), Some("1"));
    assert_eq!(snapshot.session_revision, 1);
}

#[test]
fn payload_identity_session_load_preserves_existing_project() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);
    assert!(loaded.ok);

    let mut invalid = sample_project_current();
    invalid.graph.nodes[0].implementation = Some(core_impl("object.core.bool"));
    let response = session.load_project_current(invalid);
    let snapshot = session.snapshot();

    assert!(!response.ok);
    assert!(response.snapshot.loaded());
    assert_eq!(response.snapshot.graph_revision(), Some("1"));
    assert_eq!(
        response.snapshot.session_revision,
        loaded.snapshot.session_revision
    );
    assert!(
        response
            .issues
            .iter()
            .any(|issue| issue.code.as_deref() == Some("graph.payload-node-kind"))
    );
    assert_eq!(snapshot.graph_revision(), Some("1"));
    assert_eq!(snapshot.session_revision, loaded.snapshot.session_revision);
    assert!(
        snapshot
            .project
            .as_ref()
            .unwrap()
            .graph
            .nodes
            .iter()
            .all(|node| {
                node.implementation
                    .as_ref()
                    .map(|implementation| implementation.object_id.as_str())
                    != Some("object.core.bool")
            })
    );
}

#[test]
fn registry_invalid_patch_result_does_not_mutate_session() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);

    let response = session.apply_patch(missing_definition_node_patch());
    let snapshot = session.snapshot();

    assert_graph_patch_rejected(&response);
    assert_eq!(snapshot.graph_revision(), Some("1"));
    assert_eq!(snapshot.session_revision, 1);
}

#[test]
fn remove_node_patch_removes_incident_edges() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);

    let response = session.apply_patch(graph_patch(json!({
      "schema": "skenion.graph.patch",
      "schemaVersion": "0.1.0",
      "id": "remove-node",
      "baseRevision": "1",
      "ops": [
        { "op": "removeNode", "nodeId": "value_1" }
      ]
    })));

    assert_graph_patch_rejected(&response);
    let graph = patch_graph(&response);
    assert_eq!(graph.revision, "1");
    assert!(graph.nodes.iter().any(|node| node.id == "value_1"));
    assert_eq!(graph.edges.len(), 1);
}

#[test]
fn patch_non_numeric_revision_gets_suffix() {
    let mut project = sample_project_current();
    project.graph.revision = "rev_0001".to_owned();
    let mut session = RuntimeSession::default();
    session.load_project_current(project);

    let response = session.apply_patch(set_value_patch("rev_0001", 0.75));

    assert_graph_patch_rejected(&response);
    assert_eq!(patch_graph(&response).revision, "rev_0001");
}

#[test]
fn history_starts_empty_and_undo_redo_empty_stack_returns_errors() {
    let mut session = RuntimeSession::default();

    let history = session.history();
    let undo = session.undo();
    let redo = session.redo();

    assert_eq!(history.schema, "skenion.runtime.history");
    assert!(!history.can_undo);
    assert!(!history.can_redo);
    assert!(!undo.ok);
    assert!(!undo.applied);
    assert!(latest_history_entry(&undo).is_none());
    assert!(undo.issues[0].message.contains("available to undo"));
    assert!(!redo.ok);
    assert!(!redo.applied);
    assert!(latest_history_entry(&redo).is_none());
    assert!(redo.issues[0].message.contains("available to redo"));
}

#[test]
fn undo_after_patch_restores_graph_and_records_history_entry() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let applied = session.apply_runtime_operation(paste_operation("1"));
    assert!(applied.ok);
    let apply_event_id = applied.history_entry_id.clone().unwrap();

    let undone = session.undo();

    assert!(undone.ok);
    assert!(undone.applied);
    assert_eq!(patch_graph(&undone).revision, "3");
    assert!(
        !patch_graph(&undone)
            .nodes
            .iter()
            .any(|node| node.id == "pasted_target")
    );
    assert_eq!(undone.snapshot.session_revision, 3);
    let undo_entry = latest_history_entry(&undone).unwrap();
    assert_eq!(undo_entry.kind, RuntimeHistoryEntryKind::Undo);
    assert_eq!(
        undo_entry.subject_event_id.as_deref(),
        Some(apply_event_id.as_str())
    );
    assert!(undo_entry.mutation.graph_patch.is_none());
    assert!(undo_entry.inverse_mutation.graph_patch.is_none());
    assert_eq!((undone.history.entries).len(), 2);
    assert_eq!(undone.history.undo_depth, 0);
    assert_eq!(undone.history.redo_depth, 1);
}

#[test]
fn redo_after_undo_reapplies_graph_and_records_history_entry() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    session.apply_runtime_operation(paste_operation("1"));
    session.undo();

    let redone = session.redo();

    assert!(redone.ok);
    assert!(redone.applied);
    assert_eq!(patch_graph(&redone).revision, "4");
    assert!(
        patch_graph(&redone)
            .nodes
            .iter()
            .any(|node| node.id == "pasted_target")
    );
    assert_eq!(redone.snapshot.session_revision, 4);
    let redo_entry = latest_history_entry(&redone).unwrap();
    assert_eq!(redo_entry.kind, RuntimeHistoryEntryKind::Redo);
    assert!(redo_entry.mutation.graph_patch.is_none());
    assert!(redo_entry.inverse_mutation.graph_patch.is_none());
    assert_eq!((redone.history.entries).len(), 3);
    assert_eq!(redone.history.undo_depth, 1);
    assert_eq!(redone.history.redo_depth, 0);
}

#[test]
fn view_state_patch_undo_redo_moves_once_from_start_to_end() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);
    assert!(loaded.ok);
    let start = loaded
        .snapshot
        .view_state()
        .cloned()
        .expect("loaded view state");
    let mut moved = start.clone();
    moved.canvas.nodes.get_mut("value_1").unwrap().x += 240.0;
    moved.canvas.nodes.get_mut("value_1").unwrap().y += 120.0;

    let applied = session.apply_mutation(RuntimeMutationRequest {
        graph_patch: None,
        view_patch: Some(RuntimeViewPatch {
            base_view_revision: 1,
            ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                node_id: "value_1".to_owned(),
                from: Some(start.canvas.nodes["value_1"].clone()),
                to: moved.canvas.nodes["value_1"].clone(),
            }],
        }),
        actor_id: None,
        client_id: Some("studio-a".to_owned()),
        description: Some("drag value_1".to_owned()),
    });

    assert!(applied.ok);
    assert!(applied.applied);
    let apply_entry = latest_history_entry(&applied).unwrap();
    assert_eq!(apply_entry.kind, RuntimeHistoryEntryKind::Apply);
    assert!(apply_entry.mutation.graph_patch.is_none());
    assert!(apply_entry.mutation.view_patch.is_some());
    assert_eq!(applied.history.undo_depth, 1);
    assert_eq!(applied.snapshot.view_revision, 2);
    assert_eq!(patch_view_state(&applied), &moved);

    let undone = session.undo();

    assert!(undone.ok);
    assert!(undone.applied);
    let undo_entry = latest_history_entry(&undone).unwrap();
    assert_eq!(undo_entry.kind, RuntimeHistoryEntryKind::Undo);
    assert!(undo_entry.mutation.graph_patch.is_none());
    assert!(undo_entry.mutation.view_patch.is_some());
    assert_eq!(undone.history.undo_depth, 0);
    assert_eq!(undone.history.redo_depth, 1);
    assert_eq!(undone.snapshot.view_revision, 3);
    assert_eq!(patch_view_state(&undone), &start);

    let redone = session.redo();

    assert!(redone.ok);
    assert!(redone.applied);
    let redo_entry = latest_history_entry(&redone).unwrap();
    assert_eq!(redo_entry.kind, RuntimeHistoryEntryKind::Redo);
    assert!(redo_entry.mutation.graph_patch.is_none());
    assert!(redo_entry.mutation.view_patch.is_some());
    assert_eq!(redone.history.undo_depth, 1);
    assert_eq!(redone.history.redo_depth, 0);
    assert_eq!(redone.snapshot.view_revision, 4);
    assert_eq!(patch_view_state(&redone), &moved);
}

#[test]
fn empty_and_conflicting_view_mutations_are_rejected_without_history() {
    let mut session = RuntimeSession::default();
    assert!(load_sample_project(&mut session).ok);

    let empty = session.apply_mutation(RuntimeMutationRequest {
        graph_patch: None,
        view_patch: None,
        actor_id: None,
        client_id: None,
        description: None,
    });
    let conflict = session.apply_mutation(RuntimeMutationRequest {
        graph_patch: None,
        view_patch: Some(RuntimeViewPatch {
            base_view_revision: 99,
            ops: Vec::new(),
        }),
        actor_id: None,
        client_id: None,
        description: None,
    });

    assert!(!empty.ok);
    assert!(!empty.applied);
    assert!(empty.issues[0].message.contains("did not include"));
    assert!(!conflict.ok);
    assert!(conflict.conflict);
    assert!(conflict.issues[0].message.contains("baseViewRevision"));
    assert_eq!(conflict.history.entries.len(), 0);
}

#[test]
fn view_patch_set_node_view_success_errors_and_noop_paths() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);
    assert!(loaded.ok);
    let start = loaded
        .snapshot
        .view_state()
        .cloned()
        .expect("loaded view state");
    let value_view = start.canvas.nodes["value_1"].clone();
    let mut moved_view = value_view.clone();
    moved_view.x += 80.0;

    let set = session.apply_mutation(RuntimeMutationRequest {
        graph_patch: None,
        view_patch: Some(RuntimeViewPatch {
            base_view_revision: 1,
            ops: vec![RuntimeViewPatchOperation::SetNodeView {
                node_id: "value_1".to_owned(),
                view: moved_view.clone(),
            }],
        }),
        actor_id: None,
        client_id: None,
        description: Some("set node view".to_owned()),
    });
    assert!(set.ok);
    assert!(set.applied);
    assert_eq!(patch_view_state(&set).canvas.nodes["value_1"], moved_view);

    let noop = session.apply_mutation(RuntimeMutationRequest {
        graph_patch: None,
        view_patch: Some(RuntimeViewPatch {
            base_view_revision: 2,
            ops: vec![RuntimeViewPatchOperation::SetNodeView {
                node_id: "value_1".to_owned(),
                view: moved_view.clone(),
            }],
        }),
        actor_id: None,
        client_id: None,
        description: None,
    });
    assert!(noop.ok);
    assert!(!noop.applied);

    let missing_node = session.apply_mutation(RuntimeMutationRequest {
        graph_patch: None,
        view_patch: Some(RuntimeViewPatch {
            base_view_revision: 2,
            ops: vec![RuntimeViewPatchOperation::SetNodeView {
                node_id: "missing".to_owned(),
                view: value_view.clone(),
            }],
        }),
        actor_id: None,
        client_id: None,
        description: None,
    });
    assert!(!missing_node.ok);
    assert!(missing_node.issues[0].message.contains("does not exist"));
}

#[test]
fn view_mutation_on_invalid_stored_graph_returns_issues_without_panic() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);
    assert!(loaded.ok);
    assert!(loaded.snapshot.plan.is_some());
    let start = loaded
        .snapshot
        .view_state()
        .expect("loaded view state")
        .canvas
        .nodes["value_1"]
        .clone();
    let mut moved = start.clone();
    moved.x += 24.0;

    let target_node = session
        .project
        .as_mut()
        .expect("project should remain loaded")
        .graph
        .nodes
        .iter_mut()
        .find(|node| node.id == "target_1")
        .expect("sample graph should include target node");
    let cold_port = target_node
        .ports
        .iter_mut()
        .find(|port| port.id == "cold")
        .expect("sample target should include cold inlet");
    cold_port.port_type = "value.core.bool".to_owned();

    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session.apply_mutation(RuntimeMutationRequest {
            graph_patch: None,
            view_patch: Some(RuntimeViewPatch {
                base_view_revision: loaded.snapshot.view_revision,
                ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                    node_id: "value_1".to_owned(),
                    from: Some(start),
                    to: moved,
                }],
            }),
            actor_id: None,
            client_id: Some("studio-a".to_owned()),
            description: Some("drag invalid stored graph".to_owned()),
        })
    }))
    .expect("invalid stored graph should return issues instead of panicking");

    assert!(!response.ok);
    assert!(!response.applied);
    assert!(!response.conflict);
    assert_eq!(
        response.snapshot.session_revision,
        loaded.snapshot.session_revision
    );
    assert_eq!(
        response.snapshot.view_revision,
        loaded.snapshot.view_revision
    );
    assert!(response.snapshot.plan.is_none());
    assert!(
        response.issues.iter().any(|issue| {
            issue.code.as_deref() == Some("node.port-snapshot.type-mismatch")
                && issue.message.contains(
                    "port snapshot mismatch: target_1.cold type value.core.bool != definition type value.core.float32",
                )
        })
    );
    assert!(
        response
            .snapshot
            .issues
            .iter()
            .any(|issue| { issue.code.as_deref() == Some("node.port-snapshot.type-mismatch") })
    );
    assert_eq!(response.history.entries.len(), 0);
}

#[test]
fn view_patch_helper_reports_missing_view_and_from_mismatch() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);
    assert!(loaded.ok);
    let graph = session.graph().expect("loaded graph");
    let mut view_state = loaded
        .snapshot
        .view_state()
        .cloned()
        .expect("loaded view state");
    let value_view = view_state.canvas.nodes["value_1"].clone();
    let mut moved_view = value_view.clone();
    moved_view.y += 80.0;

    view_state.canvas.nodes.remove("value_1");
    let missing_set_view = super::apply_view_patch_to_view_state(
        &graph,
        view_state.clone(),
        &RuntimeViewPatch {
            base_view_revision: 1,
            ops: vec![RuntimeViewPatchOperation::SetNodeView {
                node_id: "value_1".to_owned(),
                view: moved_view.clone(),
            }],
        },
    );
    let missing_move_view = super::apply_view_patch_to_view_state(
        &graph,
        view_state,
        &RuntimeViewPatch {
            base_view_revision: 1,
            ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                node_id: "value_1".to_owned(),
                from: None,
                to: moved_view.clone(),
            }],
        },
    );
    let missing_move_node = super::apply_view_patch_to_view_state(
        &graph,
        loaded.snapshot.view_state().cloned().unwrap(),
        &RuntimeViewPatch {
            base_view_revision: 1,
            ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                node_id: "missing".to_owned(),
                from: None,
                to: moved_view.clone(),
            }],
        },
    );

    let mut mismatched_from = value_view.clone();
    mismatched_from.x += 1.0;
    let from_mismatch = super::apply_view_patch_to_view_state(
        &graph,
        loaded.snapshot.view_state().cloned().unwrap(),
        &RuntimeViewPatch {
            base_view_revision: 1,
            ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                node_id: "value_1".to_owned(),
                from: Some(mismatched_from),
                to: moved_view,
            }],
        },
    );

    assert!(
        missing_set_view
            .unwrap_err()
            .first()
            .unwrap()
            .message
            .contains("has no view state")
    );
    assert!(
        missing_move_view
            .unwrap_err()
            .first()
            .unwrap()
            .message
            .contains("has no view state")
    );
    assert!(
        missing_move_node
            .unwrap_err()
            .first()
            .unwrap()
            .message
            .contains("does not exist")
    );
    assert!(
        from_mismatch
            .unwrap_err()
            .first()
            .unwrap()
            .message
            .contains("from view does not match")
    );
}

#[test]
fn combined_graph_and_noop_view_mutation_keeps_view_revision_stable() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);
    assert!(loaded.ok);
    let value_view = loaded.snapshot.view_state().unwrap().canvas.nodes["value_1"].clone();

    let response = session.apply_mutation(RuntimeMutationRequest {
        graph_patch: Some(set_value_patch("1", 0.5)),
        view_patch: Some(RuntimeViewPatch {
            base_view_revision: 1,
            ops: vec![RuntimeViewPatchOperation::SetNodeView {
                node_id: "value_1".to_owned(),
                view: value_view,
            }],
        }),
        actor_id: None,
        client_id: None,
        description: Some("set graph without moving view".to_owned()),
    });

    assert_graph_patch_rejected(&response);
    assert_eq!(response.snapshot.graph_revision(), Some("1"));
    assert_eq!(response.snapshot.view_revision, 1);
    assert!(latest_history_entry(&response).is_none());
}

#[test]
fn new_patch_after_undo_clears_redo_stack() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    session.apply_runtime_operation(paste_operation("1"));
    let undone = session.undo();
    assert_eq!(undone.history.redo_depth, 1);

    let applied = session.apply_runtime_operation(paste_operation("3"));

    assert!(applied.ok);
    assert_eq!(applied.revision_after.as_deref(), Some("4"));
    let history = session.history();
    assert_eq!(history.entries.len(), 3);
    assert_eq!(history.undo_depth, 1);
    assert_eq!(history.redo_depth, 0);
    assert!(!history.can_redo);
}

#[test]
fn graph_patch_remove_node_patch_is_rejected_without_history() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let removed = session.apply_patch(graph_patch(json!({
      "schema": "skenion.graph.patch",
      "schemaVersion": "0.1.0",
      "id": "remove-node",
      "baseRevision": "1",
      "ops": [
        { "op": "removeNode", "nodeId": "value_1" }
      ]
    })));

    assert_graph_patch_rejected(&removed);
    assert!(
        patch_graph(&removed)
            .nodes
            .iter()
            .any(|node| node.id == "value_1")
    );
    assert_eq!(patch_graph(&removed).edges.len(), 1);
    assert_eq!(removed.history.undo_depth, 0);
}

#[test]
fn graph_patch_connection_and_delete_patches_are_rejected_without_history() {
    let mut project = sample_project_current();
    project.graph.edges.clear();
    let mut session = RuntimeSession::default();
    assert!(session.load_project_current(project).ok);
    let connected = session.apply_patch(duplicate_edge_patch());
    assert_graph_patch_rejected(&connected);
    assert!(patch_graph(&connected).edges.is_empty());
    let deleted = session.apply_patch(graph_patch(json!({
      "schema": "skenion.graph.patch",
      "schemaVersion": "0.1.0",
      "id": "delete-target",
      "baseRevision": "2",
      "ops": [
        { "op": "removeNode", "nodeId": "target_1" }
      ]
    })));
    assert_graph_patch_rejected(&deleted);
    assert!(patch_graph(&deleted).edges.is_empty());
    assert_eq!(deleted.history.undo_depth, 0);
}

#[test]
fn multiple_undo_operations_keep_advancing_revision() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    session.apply_runtime_operation(paste_operation("1"));
    session.apply_runtime_operation(paste_operation("2"));

    let first_undo = session.undo();
    let second_undo = session.undo();

    assert!(first_undo.ok);
    assert_eq!(patch_graph(&first_undo).revision, "4");
    assert!(
        !patch_graph(&first_undo)
            .nodes
            .iter()
            .any(|node| node.id == "pasted_target_2")
    );
    assert!(second_undo.ok);
    assert_eq!(patch_graph(&second_undo).revision, "5");
    assert!(
        !patch_graph(&second_undo)
            .nodes
            .iter()
            .any(|node| node.id == "pasted_target")
    );
    assert_eq!((second_undo.history.entries).len(), 4);
    assert_eq!(second_undo.history.redo_depth, 2);
}

#[test]
fn failed_history_operations_do_not_mutate_stacks_or_session() {
    let mut no_loaded = RuntimeSession::default();
    no_loaded.undo_stack.push(HistoryEntry::Mutation {
        event_id: "event_bad".to_owned(),
        actor_id: None,
        mutation: graph_mutation(set_value_patch("1", 0.75)),
        inverse_mutation: graph_mutation(set_value_patch("1", 0.5)),
    });
    let no_loaded_response = no_loaded.undo();
    assert!(!no_loaded_response.ok);
    assert_eq!(no_loaded_response.history.undo_depth, 1);
    assert!(
        no_loaded_response.issues[0]
            .message
            .contains("no project loaded")
    );

    let mut invalid_inverse = RuntimeSession::default();
    load_sample_project(&mut invalid_inverse);
    invalid_inverse.undo_stack.push(HistoryEntry::Mutation {
        event_id: "event_bad_inverse".to_owned(),
        actor_id: None,
        mutation: graph_mutation(set_value_patch("1", 0.75)),
        inverse_mutation: graph_mutation(missing_node_patch()),
    });
    let invalid_inverse_response = invalid_inverse.undo();
    assert!(!invalid_inverse_response.ok);
    assert_eq!(invalid_inverse_response.history.undo_depth, 1);
    assert_eq!(
        invalid_inverse_response.snapshot.graph_revision(),
        Some("1")
    );

    let mut invalid_redo = RuntimeSession::default();
    load_sample_project(&mut invalid_redo);
    invalid_redo.redo_stack.push(HistoryEntry::Mutation {
        event_id: "event_bad_redo".to_owned(),
        actor_id: None,
        mutation: graph_mutation(missing_definition_node_patch()),
        inverse_mutation: graph_mutation(set_value_patch("1", 0.5)),
    });
    let invalid_redo_response = invalid_redo.redo();
    assert!(!invalid_redo_response.ok);
    assert_eq!(invalid_redo_response.history.redo_depth, 1);
    assert_eq!(
        invalid_redo_response.issues[0].code.as_deref(),
        Some("project.graph-patch-unsupported")
    );

    let mut no_actor_history = RuntimeSession::default();
    load_sample_project(&mut no_actor_history);
    let no_actor_undo = no_actor_history.undo_for_actor("participant-a");
    let no_actor_redo = no_actor_history.redo_for_actor("participant-a");
    assert!(!no_actor_undo.ok);
    assert!(no_actor_undo.issues[0].message.contains("actor"));
    assert!(!no_actor_redo.ok);
    assert!(no_actor_redo.issues[0].message.contains("actor"));

    let mut invalid_actor_inverse = RuntimeSession::default();
    load_sample_project(&mut invalid_actor_inverse);
    invalid_actor_inverse
        .undo_stack
        .push(HistoryEntry::Mutation {
            event_id: "event_bad_actor_inverse".to_owned(),
            actor_id: Some("participant-a".to_owned()),
            mutation: graph_mutation(set_value_patch("1", 0.75)),
            inverse_mutation: graph_mutation(missing_node_patch()),
        });
    let invalid_actor_inverse_response = invalid_actor_inverse.undo_for_actor("participant-a");
    assert!(!invalid_actor_inverse_response.ok);
    assert_eq!(invalid_actor_inverse_response.history.undo_depth, 1);

    let mut invalid_actor_redo = RuntimeSession::default();
    load_sample_project(&mut invalid_actor_redo);
    invalid_actor_redo.redo_stack.push(HistoryEntry::Mutation {
        event_id: "event_bad_actor_redo".to_owned(),
        actor_id: Some("participant-a".to_owned()),
        mutation: graph_mutation(missing_definition_node_patch()),
        inverse_mutation: graph_mutation(set_value_patch("1", 0.5)),
    });
    let invalid_actor_redo_response = invalid_actor_redo.redo_for_actor("participant-a");
    assert!(!invalid_actor_redo_response.ok);
    assert_eq!(invalid_actor_redo_response.history.redo_depth, 1);
}

#[test]
fn reject_patch_uses_current_session_snapshot() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);

    let response = session.reject_patch(
        false,
        vec![RuntimeIssue::error("invalid graph patch: unsupported op")],
    );

    assert!(!response.ok);
    assert!(!response.applied);
    assert!(latest_history_entry(&response).is_none());
    assert_eq!((response.history.entries).len(), 0);
    assert_eq!(patch_graph(&response).revision, "1");
    assert_eq!(response.snapshot.graph_revision(), Some("1"));
    assert!(response.issues[0].message.contains("unsupported op"));
}

#[test]
fn paste_graph_fragment_lowers_to_root_graph_mutation_with_id_remap() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);

    let response = session.apply_runtime_operation(paste_operation("1"));

    assert!(response.ok);
    assert!(response.applied);
    assert!(!response.conflict);
    assert_eq!(response.revision_before, "1");
    assert_eq!(response.revision_after.as_deref(), Some("2"));
    assert_eq!(
        response
            .id_remap
            .node_id_map
            .get("value_1")
            .map(String::as_str),
        Some("value_1_2")
    );
    assert_eq!(
        response
            .id_remap
            .edge_id_map
            .get("edge_value_to_pasted")
            .map(String::as_str),
        Some("edge_value_to_pasted")
    );
    let graph = session.graph().expect("graph should remain loaded");
    assert!(graph.nodes.iter().any(|node| node.id == "value_1_2"));
    assert!(graph.nodes.iter().any(|node| node.id == "pasted_target"));
    assert!(graph.edges.iter().any(|edge| {
        edge.from.node == "value_1_2"
            && edge.from.port == "value"
            && edge.to.node == "pasted_target"
            && edge.to.port == "cold"
    }));
    assert_eq!(
        response.history_entry_id.as_deref(),
        Some("runtime_event_000001")
    );
}

#[test]
fn payload_identity_paste_is_rejected_without_mutating_session() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);
    assert!(loaded.ok);
    let mut operation = paste_operation("1");
    operation.request.fragment.nodes[0].implementation = Some(core_impl("object.core.string"));

    let response = session.apply_runtime_operation(operation);
    let snapshot = session.snapshot();

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(response.issues[0].code, "paste.fragment.payload-node-kind");
    assert_eq!(response.revision_after, None);
    assert_eq!(snapshot.session_revision, loaded.snapshot.session_revision);
    assert_eq!(snapshot.graph_revision(), Some("1"));
    assert!(session.history().entries.is_empty());
    let graph = session.graph().expect("graph should remain loaded");
    assert!(!graph.nodes.iter().any(|node| node.id == "pasted_target"));
}

#[test]
fn paste_graph_fragment_remaps_past_existing_generated_ids() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let first = session.apply_runtime_operation(paste_operation("1"));
    assert!(first.ok);

    let second = session.apply_runtime_operation(paste_operation("2"));

    assert!(second.ok);
    assert_eq!(
        second
            .id_remap
            .node_id_map
            .get("value_1")
            .map(String::as_str),
        Some("value_1_3")
    );
}

#[test]
fn paste_graph_fragment_uses_default_descriptions_without_attribution() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.attribution = None;

    let response = session.apply_runtime_operation(operation);

    assert!(response.ok);
    let mut history = session.history();
    let entry = history.entries.pop().expect("history should exist");
    assert_eq!(
        entry.description.as_deref(),
        Some("Paste graph fragment op-paste")
    );
    assert!(entry.mutation.graph_patch.is_none());
}

#[test]
fn paste_graph_fragment_reports_no_loaded_project() {
    let mut session = RuntimeSession::default();

    let response = session.apply_runtime_operation(paste_operation("1"));

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(response.revision_before, "1");
    assert_eq!(response.issues[0].code, "paste.target.no-project");
}

#[test]
fn paste_graph_fragment_rejects_invalid_operation_envelope() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.kind = "loadProject".to_owned();

    let response = session.apply_runtime_operation(operation);

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(response.issues[0].code, "paste.operation.invalid-envelope");
    assert!(
        response.issues[0]
            .message
            .contains("unsupported runtime operation kind")
    );
}

#[test]
fn paste_graph_fragment_rejects_id_conflicts_when_requested() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.request.options = Some(skenion_contracts::PasteGraphFragmentOptions {
        outside_endpoint_policy: None,
        id_conflict_policy: Some(skenion_contracts::IdConflictPolicy::Reject),
        interface_incident_edge_policy: None,
        preserve_relative_positions: None,
    });

    let response = session.apply_runtime_operation(operation);

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(response.issues[0].code, "paste.id-conflict");
    assert_eq!(
        response.issues[0].duplicates.as_deref(),
        Some(&["value_1".to_owned()][..])
    );
    assert_eq!(
        response
            .id_remap
            .node_id_map
            .get("value_1")
            .map(String::as_str),
        Some("value_1")
    );
}

#[test]
fn paste_graph_fragment_rejects_unsupported_interface_incident_edge_policy() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.request.options = Some(skenion_contracts::PasteGraphFragmentOptions {
        outside_endpoint_policy: None,
        id_conflict_policy: Some(skenion_contracts::IdConflictPolicy::Remap),
        interface_incident_edge_policy: Some(
            skenion_contracts::InterfaceIncidentEdgePolicyV01::Reject,
        ),
        preserve_relative_positions: None,
    });

    let response = session.apply_runtime_operation(operation);

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(response.revision_after, None);
    assert!(response.id_remap.node_id_map.is_empty());
    assert_eq!(
        response.issues[0].code,
        "paste.options.unsupported-interface-incident-edge-policy"
    );
    assert_eq!(
        response.issues[0].path.as_deref(),
        Some("request.options.interfaceIncidentEdgePolicy")
    );
    assert_eq!(
        response.issues[0].interface_policy,
        Some(skenion_contracts::InterfaceIncidentEdgePolicyV01::Reject)
    );
    assert_eq!(session.snapshot().graph_revision(), Some("1"));
}

#[test]
fn paste_graph_fragment_reports_apply_mutation_failures_as_operation_issues() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.request.fragment.nodes[1].implementation = Some(core_impl("missing.kind"));

    let response = session.apply_runtime_operation(operation);

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(response.history_entry_id, None);
    assert_eq!(response.revision_after, None);
    assert_eq!(
        response.issues[0].code,
        "object-spec.implementation-mismatch"
    );
    assert!(
        response.issues[0]
            .message
            .contains("resolves to implementation")
    );
}

#[test]
fn paste_operation_validation_reports_fragment_analysis_errors() {
    let mut session = RuntimeSession::default();
    session.load_project_current(sample_project_current());
    let mut operation = paste_operation("1");
    operation.request.fragment.nodes[1].ports[1].id = "renamed".to_owned();

    let response = session.apply_runtime_operation(operation);

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(response.issues[0].code, "paste.operation.invalid-envelope");
    assert!(response.issues[0].message.contains("missing-target-port"));
    assert!(response.id_remap.node_id_map.is_empty());
}

#[test]
fn paste_lowering_skips_fragment_view_entries_missing_from_view_map() {
    let mut operation = paste_operation("1");
    operation
        .request
        .fragment
        .view
        .as_mut()
        .expect("fragment should include view")
        .nodes
        .as_mut()
        .expect("fragment view should include nodes")
        .remove("pasted_target");
    let node_id_map = [("pasted_target".to_owned(), "pasted_target".to_owned())]
        .into_iter()
        .collect();

    let patch = lower_fragment_view_patch(1, &operation.request, &node_id_map);

    assert!(patch.is_none());
}

#[test]
fn paste_lowering_handles_absent_fragment_view_and_unmapped_edge_endpoints() {
    let mut operation = paste_operation("1");
    operation.request.fragment.view = None;

    let patch = lower_fragment_view_patch(1, &operation.request, &BTreeMap::new());

    assert!(patch.is_none());

    operation.request.fragment.view = Some(skenion_contracts::GraphFragmentViewV01 { nodes: None });
    let patch = lower_fragment_view_patch(1, &operation.request, &BTreeMap::new());
    assert!(patch.is_none());

    let edge = EdgeSpecCurrent {
        id: "edge".to_owned(),
        source: EdgeEndpointCurrent {
            node_id: "outside_source".to_owned(),
            port_id: "out".to_owned(),
        },
        target: EdgeEndpointCurrent {
            node_id: "outside_target".to_owned(),
            port_id: "in".to_owned(),
        },
        resolved_type: None,
        order: None,
        enabled: None,
        adapter: None,
        feedback: None,
        style_override: None,
        label: None,
        description: None,
    };

    let remapped = remap_edge(&edge, &BTreeMap::new());

    assert_eq!(remapped.from.node, "outside_source");
    assert_eq!(remapped.to.node, "outside_target");
}

#[test]
fn runtime_issue_conversion_preserves_severity_and_code_defaults() {
    let target = skenion_contracts::GraphTargetRef {
        path: skenion_contracts::PatchPath::Root,
        base_revision: "1".to_owned(),
        target_revision: None,
    };
    let error = RuntimeIssue::error("plain error");
    let warning = RuntimeIssue {
        severity: crate::IssueSeverity::Warning,
        message: "coded warning".to_owned(),
        code: Some("runtime.warning".to_owned()),
        details: None,
    };
    let info = RuntimeIssue {
        severity: crate::IssueSeverity::Info,
        message: "info".to_owned(),
        code: Some("runtime.info".to_owned()),
        details: None,
    };

    let converted_error = runtime_issue_to_operation_issue(&error, &target);
    let converted_warning = runtime_issue_to_operation_issue(&warning, &target);
    let converted_info = runtime_issue_to_operation_issue(&info, &target);

    assert_eq!(converted_error.severity, "error");
    assert_eq!(converted_error.code, "paste.lowering.failed");
    assert_eq!(converted_warning.severity, "warning");
    assert_eq!(converted_warning.code, "runtime.warning");
    assert_eq!(converted_info.severity, "info");
    assert_eq!(converted_info.code, "runtime.info");

    let warning = super::operation_issue_to_runtime_issue(RuntimeOperationIssue {
        severity: "warning".to_owned(),
        code: "paste.warning".to_owned(),
        message: "warning".to_owned(),
        path: None,
        target: Some(target.clone()),
        expected_revision: Some("1".to_owned()),
        actual_revision: Some("2".to_owned()),
        duplicates: None,
        nodes: None,
        edges: None,
        interface_policy: None,
        interface_detail: None,
    });
    let info = super::operation_issue_to_runtime_issue(RuntimeOperationIssue {
        severity: "info".to_owned(),
        code: "paste.info".to_owned(),
        message: "info".to_owned(),
        path: None,
        target: Some(target),
        expected_revision: None,
        actual_revision: None,
        duplicates: None,
        nodes: None,
        edges: None,
        interface_policy: None,
        interface_detail: None,
    });
    assert_eq!(warning.severity, crate::IssueSeverity::Warning);
    assert_eq!(warning.code.as_deref(), Some("paste.warning"));
    assert_eq!(info.severity, crate::IssueSeverity::Info);
    assert_eq!(info.code.as_deref(), Some("paste.info"));
}

#[test]
fn rejected_collaboration_edge_connect_preserves_session_graph() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);
    assert!(loaded.ok);
    let target = paste_operation("1").request.target;

    let response = session.apply_collaboration_change_set_current(
        target,
        vec![collaboration_change(json!({
          "op": "edge.connect",
          "changeId": "connect-output-to-output",
          "edge": {
            "id": "edge_invalid_direction",
            "source": { "nodeId": "value_1", "portId": "value" },
            "target": { "nodeId": "target_1", "portId": "value" }
          }
        }))],
        None,
        None,
        None,
    );
    let snapshot = session.snapshot();

    assert!(!response.ok);
    assert!(!response.applied);
    assert!(
        response
            .issues
            .iter()
            .any(|issue| issue.code.as_deref() == Some("graph.edge-target-direction"))
    );
    assert_eq!(snapshot.session_revision, loaded.snapshot.session_revision);
    assert_eq!(snapshot.graph_revision(), Some("1"));
    assert!(session.history().entries.is_empty());
    let graph = session.graph().expect("graph should remain loaded");
    assert!(graph.edges.iter().all(|edge| {
        !(edge.from.node == "value_1"
            && edge.from.port == "value"
            && edge.to.node == "target_1"
            && edge.to.port == "value")
    }));
    assert_eq!(graph.edges.len(), 1);
}

#[test]
fn rejected_payload_identity_collaboration_node_add_preserves_session_graph() {
    let mut session = RuntimeSession::default();
    let loaded = load_sample_project(&mut session);
    assert!(loaded.ok);
    let target = paste_operation("1").request.target;

    let response = session.apply_collaboration_change_set_current(
        target,
        vec![collaboration_change(json!({
          "op": "node.add",
          "changeId": "add-payload-identity",
          "node": {
            "id": "payload_identity",
            "kind": "string",
            "kindVersion": "0.1.0",
            "params": {},
            "ports": []
          }
        }))],
        None,
        None,
        None,
    );
    let snapshot = session.snapshot();

    assert!(!response.ok);
    assert!(!response.applied);
    assert!(
        response
            .issues
            .iter()
            .any(|issue| issue.code.as_deref() == Some("graph.payload-node-kind"))
    );
    assert_eq!(snapshot.session_revision, loaded.snapshot.session_revision);
    assert_eq!(snapshot.graph_revision(), Some("1"));
    assert!(session.history().entries.is_empty());
    assert!(
        session
            .graph()
            .unwrap()
            .nodes
            .iter()
            .all(|node| node.id != "payload_identity")
    );
}

#[test]
fn current_active_cutover_private_helpers_cover_defensive_paths() {
    let root_target = paste_operation("1").request.target;
    let change: RuntimeCollaborationChange = current_fixture(
        json!({
          "op": "node.add",
          "changeId": "change-add-duplicate-value",
          "node": {
            "id": "value_1",
            "kind": "object.core.float",
            "kindVersion": "0.1.0",
            "params": {},
            "ports": value_f32_ports_current_json()
          }
        }),
        "collaboration change should parse",
    );

    let mut unloaded = RuntimeSession::default();
    let no_project = unloaded.apply_collaboration_change_set_current(
        root_target.clone(),
        vec![change.clone()],
        None,
        None,
        None,
    );
    assert_eq!(
        no_project.issues[0].code.as_deref(),
        Some("collaboration.target.no-project")
    );

    let mut invalid_request = sample_project_current();
    let mut invalid_document = super::project_document_from_request_current(&invalid_request);
    invalid_document.schema_version = "9.9.9".to_owned();
    invalid_request.document = Some(invalid_document);
    let invalid_document_response = unloaded.load_project_current(invalid_request);
    assert_eq!(
        invalid_document_response.issues[0].code.as_deref(),
        Some("project.unsupported-schema-version")
    );
    assert_eq!(
        invalid_document_response.issues[0]
            .details
            .as_ref()
            .unwrap()["surface"],
        "project"
    );
    assert_eq!(
        invalid_document_response.issues[0]
            .details
            .as_ref()
            .unwrap()["expectedSchemaVersion"],
        "0.1.0"
    );
    assert_eq!(
        invalid_document_response.issues[0]
            .details
            .as_ref()
            .unwrap()["receivedSchemaVersion"],
        "9.9.9"
    );

    let mut session = RuntimeSession::default();
    assert!(session.load_project_current(sample_project_current()).ok);
    assert!(session.project_document_current().is_some());
    assert_eq!(
        session.target_revision_current(&root_target).as_deref(),
        Some("1")
    );

    let mut stale_target = root_target.clone();
    stale_target.base_revision = "0".to_owned();
    let stale = session.apply_collaboration_change_set_current(
        stale_target,
        vec![change.clone()],
        None,
        None,
        None,
    );
    assert!(stale.conflict);
    assert_eq!(
        stale.issues[0].code.as_deref(),
        Some("collaboration.revision-conflict")
    );

    let missing_help_target: skenion_contracts::GraphTargetRef = serde_json::from_value(json!({
      "path": { "kind": "help-working-copy", "workingCopyId": "missing-help" },
      "baseRevision": "1"
    }))
    .expect("target should parse");
    let missing_target = session.apply_collaboration_change_set_current(
        missing_help_target,
        vec![change.clone()],
        None,
        None,
        None,
    );
    assert_eq!(
        missing_target.issues[0].code.as_deref(),
        Some("paste.target.missing-help-working-copy")
    );

    let duplicate = session.apply_collaboration_change_set_current(
        root_target.clone(),
        vec![change],
        None,
        None,
        None,
    );
    assert_eq!(
        duplicate.issues[0].code.as_deref(),
        Some("collaboration.node-id-conflict")
    );

    let unsupported_target: skenion_contracts::GraphTargetRef = serde_json::from_value(json!({
      "path": {
        "kind": "package-patch-definition",
        "packageId": "pkg",
        "patchId": "help"
      },
      "baseRevision": "1"
    }))
    .expect("target should parse");
    let paste_error = super::paste_graph_fragment_into_project_current(
        super::project_document_from_request_current(&sample_project_current()),
        1,
        &PasteGraphFragmentRequest {
            target: unsupported_target,
            fragment: paste_operation("1").request.fragment,
            placement: None,
            options: None,
        },
    )
    .expect_err("package patch target should not be mutable");
    assert_eq!(paste_error.0[0].code, "paste.target.unsupported");

    let mut unresolved_graph = sample_project_current().graph;
    unresolved_graph.nodes.push(current_fixture(
        current_unresolved_node_json("missing_object", "missing.object"),
        "unresolved current 0.1 node should parse",
    ));
    let unresolved = super::unresolved_object_issues_current(&unresolved_graph);
    assert!(unresolved[0].message.contains("missing.object"));
}

#[test]
fn active_current_failure_paths_cover_registry_restore_and_history_rejection() {
    let mut duplicate_request = sample_project_current();
    let mut conflicting_definition = duplicate_request.nodes[0].clone();
    conflicting_definition.ports[0].port_type = "value.core.bool".to_owned();
    duplicate_request.nodes.push(conflicting_definition);
    let mut duplicate_session = RuntimeSession::default();
    let duplicate_load = duplicate_session.load_project_current(duplicate_request);
    assert!(!duplicate_load.ok);
    assert!(
        duplicate_load.issues[0]
            .message
            .contains("duplicate node definition")
    );

    let mut invalid_request = sample_project_current();
    let mut invalid_document = super::project_document_from_request_current(&invalid_request);
    invalid_document.graph.nodes[0].implementation = Some(core_impl("missing.kind"));
    invalid_request.document = Some(invalid_document);
    let mut invalid_session = RuntimeSession::default();

    let response = invalid_session.load_project_current(invalid_request);
    assert!(!response.ok);
    assert!(response.snapshot.plan.is_none());
    assert_eq!(
        response.issues[0].code.as_deref(),
        Some("object-spec.implementation-mismatch")
    );

    let mut update_session = RuntimeSession::default();
    let loaded = update_session.load_project_current(sample_project_current());
    assert!(loaded.ok, "{:?}", loaded.issues);
    let before = update_session
        .project_document_current()
        .expect("project should load");
    let mut after = before.clone();
    after.graph.revision = "2".to_owned();
    after.revision = "2".to_owned();
    update_session
        .nodes_current
        .push(conflicting_node_definition(
            &update_session.nodes_current[0],
        ));
    let update = update_session.apply_project_document_update(
        before,
        after,
        1,
        described_runtime_mutation("apply described project document"),
        None,
    );
    assert!(!update.ok);
    assert!(
        update.issues[0]
            .message
            .contains("duplicate node definition")
    );

    let mut restore_plan_session = RuntimeSession::default();
    let loaded = restore_plan_session.load_project_current(sample_project_current());
    assert!(loaded.ok, "{:?}", loaded.issues);
    let mut invalid_restore = restore_plan_session
        .project_document_current()
        .expect("project should load");
    invalid_restore.graph.nodes[0].implementation = Some(core_impl("missing.kind"));
    let restored = restore_plan_session.restore_project_document_state(
        invalid_restore,
        1,
        RuntimeHistoryEntryKind::Undo,
        described_runtime_mutation("restore invalid project"),
        empty_runtime_mutation(),
        None,
    );
    assert!(!restored.ok);
    assert_eq!(
        restored.issues[0].code.as_deref(),
        Some("object-spec.implementation-mismatch")
    );

    let mut restore_registry_session = RuntimeSession::default();
    let loaded = restore_registry_session.load_project_current(sample_project_current());
    assert!(loaded.ok, "{:?}", loaded.issues);
    let restore_project = restore_registry_session
        .project_document_current()
        .expect("project should load");
    restore_registry_session
        .nodes_current
        .push(conflicting_node_definition(
            &restore_registry_session.nodes_current[0],
        ));
    let restored = restore_registry_session.restore_project_document_state(
        restore_project,
        1,
        RuntimeHistoryEntryKind::Redo,
        described_runtime_mutation("restore registry project"),
        empty_runtime_mutation(),
        None,
    );
    assert!(!restored.ok);
    assert!(
        restored.issues[0]
            .message
            .contains("duplicate node definition")
    );

    let mut history_session = RuntimeSession::default();
    assert!(
        history_session
            .load_project_current(sample_project_current())
            .ok
    );
    let before = history_session
        .project_document_current()
        .expect("project should load");
    let mut after = before.clone();
    after.graph.nodes[0].implementation = Some(core_impl("missing.kind"));
    let entry = HistoryEntry::ProjectDocument {
        event_id: "event".to_owned(),
        actor_id: None,
        before: Box::new(before),
        after: Box::new(after),
        before_view_revision: 1,
        after_view_revision: 2,
        mutation: empty_runtime_mutation(),
        inverse_mutation: empty_runtime_mutation(),
    };

    let outcome = history_session.apply_history_entry(entry, super::HistoryDirection::Redo);
    assert!(!outcome.applied);

    let mut view_session = RuntimeSession::default();
    let loaded = view_session.load_project_current(sample_project_current());
    assert!(loaded.ok);
    let start = loaded
        .snapshot
        .view_state()
        .expect("current 0.1 load should include view state")
        .canvas
        .nodes["value_1"]
        .clone();
    let mut moved = start.clone();
    moved.x += 12.0;
    let view_patch = view_session.apply_mutation(RuntimeMutationRequest {
        graph_patch: None,
        view_patch: Some(RuntimeViewPatch {
            base_view_revision: 1,
            ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                node_id: "value_1".to_owned(),
                from: Some(start),
                to: moved,
            }],
        }),
        actor_id: None,
        client_id: None,
        description: Some("current 0.1 active view move".to_owned()),
    });
    assert!(view_patch.ok);
    assert!(view_patch.applied);

    let mut mutation = graph_mutation(set_value_patch("old", 1.0));
    mutation.view_patch = Some(RuntimeViewPatch {
        base_view_revision: 1,
        ops: Vec::new(),
    });
    super::normalize_mutation_base_revisions(&mut mutation, "graph-new".to_owned(), 9);
    assert_eq!(
        mutation
            .graph_patch
            .as_ref()
            .map(|patch| patch.base_revision.as_str()),
        Some("graph-new")
    );
    assert_eq!(
        mutation
            .view_patch
            .as_ref()
            .map(|patch| patch.base_view_revision),
        Some(9)
    );
}

#[test]
fn history_delta_helpers_merge_non_top_project_patch_and_view_edits() {
    let mut before = super::project_document_from_request_current(&sample_project_current());
    before.patch_library = vec![
        patch_definition_current("identity"),
        patch_definition_current("before-only"),
    ];
    before.view_state.canvas.nodes.insert(
        "before_only_view".to_owned(),
        crate::CanvasNodeView {
            x: 11.0,
            y: 12.0,
            width: None,
            height: None,
            collapsed: None,
        },
    );

    let mut after = before.clone();
    after.graph.nodes.push(graph_node_current("root_added"));
    after.graph.revision = "2".to_owned();
    after.revision = "2".to_owned();
    after.view_state.canvas.nodes.insert(
        "root_added".to_owned(),
        crate::CanvasNodeView {
            x: 400.0,
            y: 96.0,
            width: None,
            height: None,
            collapsed: None,
        },
    );
    after.view_state.canvas.nodes.remove("before_only_view");
    after
        .patch_library
        .retain(|patch| patch.id != "before-only");
    after.patch_library[0]
        .graph
        .nodes
        .push(graph_node_current("patch_added"));
    after.patch_library[0].graph.revision = "2".to_owned();
    after.patch_library[0].revision = "2".to_owned();

    let mut current = after.clone();
    current
        .graph
        .nodes
        .push(graph_node_current("other_actor_root"));
    current
        .patch_library
        .push(patch_definition_current("current-only"));
    current
        .patch_library
        .push(patch_definition_current("before-only"));
    current.patch_library[0]
        .graph
        .nodes
        .push(graph_node_current("other_actor_patch"));
    current.view_state.canvas.nodes.insert(
        "other_actor_root".to_owned(),
        crate::CanvasNodeView {
            x: 800.0,
            y: 96.0,
            width: None,
            height: None,
            collapsed: None,
        },
    );

    let undone = super::project_document_history_delta(
        &current,
        &before,
        &after,
        super::HistoryDirection::Undo,
    );
    assert!(
        !undone
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "root_added")
    );
    assert!(
        undone
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "other_actor_root")
    );
    assert!(
        !undone.patch_library[0]
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "patch_added")
    );
    assert!(
        undone.patch_library[0]
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "other_actor_patch")
    );
    assert!(
        undone
            .view_state
            .canvas
            .nodes
            .contains_key("other_actor_root")
    );

    let redone = super::project_document_history_delta(
        &undone,
        &before,
        &after,
        super::HistoryDirection::Redo,
    );
    assert!(
        redone
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "root_added")
    );
    assert!(
        redone
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "other_actor_root")
    );
    assert!(
        redone.patch_library[0]
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "patch_added")
    );

    let mut before_graph = before.graph.clone();
    before_graph.nodes.push(graph_node_current("before_only"));
    let mut after_graph = before_graph.clone();
    after_graph.nodes.retain(|node| node.id != "before_only");
    let mut current_graph = after_graph.clone();
    current_graph.nodes.push(graph_node_current("before_only"));
    current_graph
        .nodes
        .push(graph_node_current("not_in_before"));
    assert!(super::undo_graph_history_delta_current(
        &mut current_graph.clone(),
        &before_graph,
        &after_graph
    ));
    assert!(super::redo_graph_history_delta_current(
        &mut current_graph,
        &before_graph,
        &after_graph
    ));

    let _ = super::view_state_history_delta_current(
        &before.view_state,
        &before.view_state,
        &after.view_state,
        super::HistoryDirection::Undo,
    );
    let _ = super::view_state_history_delta_current(
        &before.view_state,
        &before.view_state,
        &after.view_state,
        super::HistoryDirection::Redo,
    );
}

#[test]
fn paste_private_helpers_cover_fragment_and_edge_conflict_errors() {
    let graph = sample_project_current().graph;
    let mut invalid_fragment = paste_operation("1").request;
    invalid_fragment.fragment.edges[0].target.node_id = "outside".to_owned();
    let invalid = super::paste_graph_fragment_into_graph_current(graph.clone(), &invalid_fragment)
        .expect_err("outside endpoint should fail analysis");
    assert_eq!(
        invalid.0[0].code,
        "paste.fragment.fragment-edge-outside-selection"
    );

    let mut edge_conflict = paste_operation("1").request;
    edge_conflict.options = Some(skenion_contracts::PasteGraphFragmentOptions {
        outside_endpoint_policy: None,
        id_conflict_policy: Some(skenion_contracts::IdConflictPolicy::Reject),
        interface_incident_edge_policy: None,
        preserve_relative_positions: None,
    });
    edge_conflict.fragment.nodes[0].id = "new_value".to_owned();
    edge_conflict.fragment.nodes[1].id = "new_target".to_owned();
    edge_conflict.fragment.edges[0].id = "edge_value_target".to_owned();
    edge_conflict.fragment.edges[0].source.node_id = "new_value".to_owned();
    edge_conflict.fragment.edges[0].target.node_id = "new_target".to_owned();
    let edge_conflict = super::paste_graph_fragment_into_graph_current(graph, &edge_conflict)
        .expect_err("duplicate edge id should fail");
    assert_eq!(edge_conflict.0[0].code, "paste.edge-id-conflict");
    assert_eq!(edge_conflict.1.edge_id_map.get("edge_value_target"), None);

    let mut used_edges = HashSet::new();
    used_edges.insert("edge_2".to_owned());
    assert_eq!(super::next_available_edge_id("edge", &used_edges), "edge_3");

    let mut unsupported = paste_operation("1").request;
    unsupported.target.path = skenion_contracts::PatchPath::EmbeddedPatchInstance {
        owner_path: vec!["root".to_owned()],
        node_id: "subpatch".to_owned(),
    };
    let unsupported = super::paste_graph_fragment_into_project_current(
        super::project_document_from_request_current(&sample_project_current()),
        1,
        &unsupported,
    )
    .expect_err("embedded patch target should fail");
    assert_eq!(unsupported.0[0].code, "paste.target.unsupported");

    let mut missing_graph = paste_operation("1").request;
    missing_graph.target.path = skenion_contracts::PatchPath::ProjectPatchDefinition {
        patch_id: "missing".to_owned(),
    };
    let missing_graph = super::paste_graph_fragment_into_project_current(
        super::project_document_from_request_current(&sample_project_current()),
        1,
        &missing_graph,
    )
    .expect_err("missing project patch should fail");
    assert_eq!(missing_graph.0[0].code, "paste.target.missing-graph");

    let mut project_with_patch =
        super::project_document_from_request_current(&sample_project_current());
    project_with_patch
        .patch_library
        .push(patch_definition_current("identity"));
    let mut patch_paste = paste_operation("1").request;
    patch_paste.target.path = skenion_contracts::PatchPath::ProjectPatchDefinition {
        patch_id: "identity".to_owned(),
    };
    let (patched_project, _, _, revision_after) =
        super::paste_graph_fragment_into_project_current(project_with_patch, 1, &patch_paste)
            .expect("project patch paste should apply");
    assert_eq!(revision_after, "2");
    assert_eq!(patched_project.patch_library[0].revision, "2");

    let remapped = super::remap_edge_current(
        &EdgeSpecCurrent {
            id: "edge".to_owned(),
            source: EdgeEndpointCurrent {
                node_id: "outside_source".to_owned(),
                port_id: "out".to_owned(),
            },
            target: EdgeEndpointCurrent {
                node_id: "outside_target".to_owned(),
                port_id: "in".to_owned(),
            },
            resolved_type: None,
            order: None,
            enabled: None,
            adapter: None,
            feedback: None,
            style_override: None,
            label: None,
            description: None,
        },
        &BTreeMap::new(),
        "edge_2".to_owned(),
    );
    assert_eq!(remapped.source.node_id, "outside_source");
    assert_eq!(remapped.target.node_id, "outside_target");
    assert_eq!(super::next_graph_revision("2"), "3");
    assert_eq!(super::next_graph_revision("rev"), "rev+1");
}

#[test]
fn collaboration_private_helpers_cover_patch_target_error_matrix() {
    let mut project = super::project_document_from_request_current(&sample_project_current());
    project
        .patch_library
        .push(patch_definition_current("identity"));
    let root_target = skenion_contracts::GraphTargetRef {
        path: skenion_contracts::PatchPath::Root,
        base_revision: "1".to_owned(),
        target_revision: None,
    };
    let patch_target = skenion_contracts::GraphTargetRef {
        path: skenion_contracts::PatchPath::ProjectPatchDefinition {
            patch_id: "identity".to_owned(),
        },
        base_revision: "1".to_owned(),
        target_revision: None,
    };

    let patch_view_error = super::apply_collaboration_changes_to_project_current(
        project.clone(),
        1,
        &patch_target,
        &[collaboration_change(json!({
          "op": "node.add",
          "changeId": "add-with-view",
          "node": value_node_current_json("patch_added"),
          "view": { "x": 1.0, "y": 2.0 }
        }))],
    )
    .expect_err("patch definition views are not active Runtime state");
    assert_eq!(
        patch_view_error[0].code.as_deref(),
        Some("collaboration.patch-view-unsupported")
    );

    let patch_add = super::apply_collaboration_changes_to_project_current(
        project.clone(),
        1,
        &patch_target,
        &[collaboration_change(json!({
          "op": "node.add",
          "changeId": "add-patch-node",
          "node": value_node_current_json("patch_added_without_view")
        }))],
    )
    .expect("patch definition node add without view should apply");
    assert_eq!(patch_add.0.patch_library[0].revision, "2");

    let patch_move_error = super::apply_collaboration_changes_to_project_current(
        project.clone(),
        1,
        &patch_target,
        &[collaboration_change(json!({
          "op": "node.move",
          "changeId": "move-patch-node",
          "nodeId": "patch_value",
          "to": { "x": 1.0, "y": 2.0 }
        }))],
    )
    .expect_err("patch definition move view should fail");
    assert_eq!(
        patch_move_error[0].code.as_deref(),
        Some("collaboration.patch-view-unsupported")
    );

    let missing_move = super::apply_collaboration_changes_to_project_current(
        project.clone(),
        1,
        &root_target,
        &[collaboration_change(json!({
          "op": "node.move",
          "changeId": "move-missing",
          "nodeId": "missing",
          "to": { "x": 1.0, "y": 2.0 }
        }))],
    )
    .expect_err("moving a missing node should fail");
    assert_eq!(
        missing_move[0].code.as_deref(),
        Some("collaboration.node-missing")
    );

    let view_conflict = super::apply_collaboration_changes_to_project_current(
        project.clone(),
        1,
        &root_target,
        &[collaboration_change(json!({
          "op": "node.move",
          "changeId": "move-conflict",
          "nodeId": "value_1",
          "from": { "x": -1.0, "y": -1.0 },
          "to": { "x": 1.0, "y": 2.0 }
        }))],
    )
    .expect_err("move from mismatch should fail");
    assert_eq!(
        view_conflict[0].code.as_deref(),
        Some("collaboration.view-conflict")
    );

    let missing_delete = super::apply_collaboration_changes_to_project_current(
        project.clone(),
        1,
        &root_target,
        &[collaboration_change(json!({
          "op": "node.delete",
          "changeId": "delete-missing",
          "nodeId": "missing"
        }))],
    )
    .expect_err("deleting a missing node should fail");
    assert_eq!(
        missing_delete[0].code.as_deref(),
        Some("collaboration.node-missing")
    );

    let duplicate_edge = super::apply_collaboration_changes_to_project_current(
        project.clone(),
        1,
        &root_target,
        &[collaboration_change(json!({
          "op": "edge.connect",
          "changeId": "connect-duplicate",
          "edge": {
            "id": "edge_value_target",
            "source": { "nodeId": "value_1", "portId": "value" },
            "target": { "nodeId": "target_1", "portId": "cold" }
          }
        }))],
    )
    .expect_err("duplicate edge id should fail");
    assert_eq!(
        duplicate_edge[0].code.as_deref(),
        Some("collaboration.edge-id-conflict")
    );

    let missing_graph_target = skenion_contracts::GraphTargetRef {
        path: skenion_contracts::PatchPath::HelpWorkingCopy {
            working_copy_id: "missing-help".to_owned(),
            source_package_id: None,
            source_patch_id: None,
        },
        base_revision: "1".to_owned(),
        target_revision: None,
    };
    let missing_graph = super::apply_collaboration_changes_to_project_current(
        project.clone(),
        1,
        &missing_graph_target,
        &[],
    )
    .expect_err("missing help graph should fail");
    assert_eq!(
        missing_graph[0].code.as_deref(),
        Some("collaboration.target.missing-graph")
    );

    let unsupported_target = skenion_contracts::GraphTargetRef {
        path: skenion_contracts::PatchPath::PackagePatchDefinition {
            package_id: "pkg".to_owned(),
            patch_id: "help".to_owned(),
            version: None,
        },
        base_revision: "1".to_owned(),
        target_revision: None,
    };
    let unsupported =
        super::apply_collaboration_changes_to_project_current(project, 1, &unsupported_target, &[])
            .expect_err("package patch target should fail");
    assert_eq!(
        unsupported[0].code.as_deref(),
        Some("collaboration.target.unsupported")
    );

    let mut unresolved_graph = sample_project_current().graph;
    unresolved_graph.nodes.push(current_fixture(
        current_unresolved_node_json("unresolved_current", "user.manipulator"),
        "unresolved current 0.1 node should parse",
    ));
    let unresolved = super::unresolved_object_issues_current(&unresolved_graph);
    assert!(unresolved[0].message.contains("unresolved object"));
}

#[test]
fn node_current_operations_report_no_project_without_mutation() {
    let target = skenion_contracts::GraphTargetRef {
        path: skenion_contracts::PatchPath::Root,
        base_revision: "1".to_owned(),
        target_revision: None,
    };
    let assert_code = |response: &RuntimePatchResponse, code: &str| {
        assert_eq!(response.issues[0].code.as_deref(), Some(code));
    };
    let mut session = RuntimeSession::default();
    let create =
        session.apply_object_node_create_current(super::ApplyObjectNodeCreateCurrentRequest {
            target: target.clone(),
            node: graph_node_current("created_value"),
            view: None,
            definition: None,
            mutation: empty_runtime_mutation(),
        });
    assert_code(&create, "node.target.no-project");

    let (replace, dropped_replace_edges) =
        session.apply_object_node_replace_current(super::ApplyObjectNodeReplaceCurrentRequest {
            target: target.clone(),
            node: graph_node_current("value_1"),
            view: None,
            definition: None,
            interface_incident_edge_policy: None,
            mutation: empty_runtime_mutation(),
        });
    assert_code(&replace, "node.target.no-project");
    assert!(dropped_replace_edges.is_empty());

    let (delete, dropped_delete_edges) = session.apply_node_delete_current(
        target.clone(),
        "value_1".to_owned(),
        None,
        Some("client-test".to_owned()),
        Some("delete without project".to_owned()),
    );
    assert_code(&delete, "node.target.no-project");
    assert!(dropped_delete_edges.is_empty());

    let update_empty = session.apply_node_update_current(
        target.clone(),
        "value_1".to_owned(),
        serde_json::Map::new(),
        None,
        Some("client-test".to_owned()),
        Some("empty update without project".to_owned()),
    );
    assert_code(&update_empty, "node.update.params-required");

    let mut params = serde_json::Map::new();
    params.insert("value".to_owned(), json!(1.0));
    let update_no_project = session.apply_node_update_current(
        target,
        "value_1".to_owned(),
        params,
        None,
        Some("client-test".to_owned()),
        Some("update without project".to_owned()),
    );
    assert_code(&update_no_project, "node.target.no-project");
}

#[test]
fn runtime_value_format_contract_accepts_supported_value_ports() {
    let typed_ports = [
        ("value.core.message", None),
        ("value.core.float32", Some("f32")),
        ("value.core.int32", Some("i32")),
        ("value.core.uint32", Some("u32")),
        ("value.core.bool", None),
        ("value.core.string", None),
        ("value.core.color", Some("rgba32f")),
        ("value.core.bang", None),
        ("value.custom.vector3", None),
    ];

    for (port_type, expected_format) in typed_ports {
        let value_format = value_format_for_port_type(port_type)
            .unwrap_or_else(|| panic!("{port_type} should map to a runtime value format"));
        assert_eq!(value_format.value_type_id, port_type);
        assert_eq!(value_format.format.as_deref(), expected_format);
    }

    assert!(value_format_for_port_type("control.number.float").is_none());
    assert!(value_format_for_port_type("asset.video").is_none());
}

#[test]
fn runtime_value_format_labels_cover_numeric_wire_formats() {
    let labels = [
        ("value.core.float16", Some("f16")),
        ("value.core.float32", Some("f32")),
        ("value.core.float64", Some("f64")),
        ("value.core.ufloat8", Some("ufloat8")),
        ("value.core.ufloat16", Some("ufloat16")),
        ("value.core.ufloat32", Some("ufloat32")),
        ("value.core.ufloat64", Some("ufloat64")),
        ("value.core.int8", Some("i8")),
        ("value.core.int16", Some("i16")),
        ("value.core.int32", Some("i32")),
        ("value.core.int64", Some("i64")),
        ("value.core.uint8", Some("u8")),
        ("value.core.uint16", Some("u16")),
        ("value.core.uint32", Some("u32")),
        ("value.core.uint64", Some("u64")),
        ("value.core.color", Some("rgba32f")),
        ("value.core.message", None),
    ];

    for (value_type_id, expected_label) in labels {
        assert_eq!(runtime_value_format_label(value_type_id), expected_label);
    }
}

#[test]
fn runtime_binding_format_revision_falls_back_to_one() {
    assert_eq!(runtime_binding_format_revision("42"), 42);
    assert_eq!(runtime_binding_format_revision("0"), 1);
    assert_eq!(runtime_binding_format_revision("not-a-number"), 1);
}

#[test]
fn paste_graph_fragment_applies_position_and_anchor_placement() {
    let mut positioned = RuntimeSession::default();
    load_sample_project(&mut positioned);
    let mut position_operation = paste_operation("1");
    position_operation.request.placement =
        Some(skenion_contracts::PastePlacement::Position { x: 300.0, y: 400.0 });

    let position_response = positioned.apply_runtime_operation(position_operation);

    assert!(position_response.ok);
    let position_view = positioned.view_state().expect("view state should exist");
    let pasted_value_view = position_view
        .canvas
        .nodes
        .get("value_1_2")
        .expect("pasted value view should exist");
    let pasted_target_view = position_view
        .canvas
        .nodes
        .get("pasted_target")
        .expect("pasted target view should exist");
    assert_eq!((pasted_value_view.x, pasted_value_view.y), (300.0, 400.0));
    assert_eq!((pasted_target_view.x, pasted_target_view.y), (470.0, 400.0));

    let mut anchored = RuntimeSession::default();
    load_sample_project(&mut anchored);
    let mut anchor_operation = paste_operation("1");
    anchor_operation.request.placement = Some(skenion_contracts::PastePlacement::Anchor {
        node_id: "value_1".to_owned(),
        offset_x: Some(16.0),
        offset_y: Some(32.0),
    });

    let anchor_response = anchored.apply_runtime_operation(anchor_operation);

    assert!(anchor_response.ok);
    let anchor_view = anchored.view_state().expect("view state should exist");
    let pasted_value_view = anchor_view
        .canvas
        .nodes
        .get("value_1_2")
        .expect("pasted value view should exist");
    assert_eq!((pasted_value_view.x, pasted_value_view.y), (26.0, 52.0));
}

#[test]
fn paste_graph_fragment_reports_base_revision_conflict() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);

    let response = session.apply_runtime_operation(paste_operation("0"));

    assert!(!response.ok);
    assert!(!response.applied);
    assert!(response.conflict);
    assert_eq!(response.revision_before, "1");
    assert_eq!(response.revision_after, None);
    assert_eq!(response.issues[0].code, "paste.revision-conflict");
    assert_eq!(session.graph().unwrap().revision, "1");
}

#[test]
fn paste_graph_fragment_rejects_missing_help_working_copy_target() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.request.target.path = skenion_contracts::PatchPath::HelpWorkingCopy {
        working_copy_id: "missing-help-copy".to_owned(),
        source_package_id: Some("skenion.core".to_owned()),
        source_patch_id: Some("float-help".to_owned()),
    };

    let response = session.apply_runtime_operation(operation);

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(
        response.issues[0].code,
        "paste.target.missing-help-working-copy"
    );
}

#[test]
fn paste_graph_fragment_allows_loaded_help_working_copy_target() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.request.target.path = skenion_contracts::PatchPath::HelpWorkingCopy {
        working_copy_id: "minimal-value".to_owned(),
        source_package_id: Some("skenion.core".to_owned()),
        source_patch_id: Some("float-help".to_owned()),
    };

    let response = session.apply_runtime_operation(operation);

    assert!(response.ok);
    assert!(response.applied);
    assert_eq!(response.revision_after.as_deref(), Some("2"));
    assert_eq!(
        response
            .id_remap
            .node_id_map
            .get("value_1")
            .map(String::as_str),
        Some("value_1_2")
    );
}

#[test]
fn paste_graph_fragment_rejects_project_patch_definition_target() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.request.target.path = skenion_contracts::PatchPath::ProjectPatchDefinition {
        patch_id: "identity".to_owned(),
    };

    let response = session.apply_runtime_operation(operation);

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(
        response.issues[0].code,
        "paste.target.missing-project-patch-definition"
    );
    assert!(
        response.issues[0]
            .message
            .contains("project patch definition identity is not loaded")
    );
}

#[test]
fn paste_graph_fragment_rejects_embedded_patch_instance_target() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.request.target.path = skenion_contracts::PatchPath::EmbeddedPatchInstance {
        owner_path: vec!["root".to_owned()],
        node_id: "subpatch_1".to_owned(),
    };

    let response = session.apply_runtime_operation(operation);

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(
        response.issues[0].code,
        "paste.target.unsupported-embedded-patch-instance"
    );
}

#[test]
fn paste_graph_fragment_rejects_outside_endpoint_by_default() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.request.fragment.edges[0].target.node_id = "outside".to_owned();

    let response = session.apply_runtime_operation(operation);

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(response.issues[0].code, "paste.operation.invalid-envelope");
    assert!(
        response.issues[0]
            .message
            .contains("fragment-edge-outside-selection")
    );
}

#[test]
fn paste_graph_fragment_omits_outside_endpoint_when_requested() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.request.fragment.edges[0].target.node_id = "outside".to_owned();
    operation.request.options = Some(skenion_contracts::PasteGraphFragmentOptions {
        outside_endpoint_policy: Some(
            skenion_contracts::GraphFragmentOutsideEndpointPolicyV01::Omit,
        ),
        id_conflict_policy: Some(skenion_contracts::IdConflictPolicy::Remap),
        interface_incident_edge_policy: None,
        preserve_relative_positions: Some(true),
    });

    let response = session.apply_runtime_operation(operation);

    assert!(response.ok);
    assert!(response.applied);
    assert_eq!(
        response.id_remap.omitted_edge_ids,
        vec!["edge_value_to_pasted"]
    );
    let graph = session.graph().unwrap();
    assert!(!graph.edges.iter().any(|edge| edge.to.node == "outside"));
}

#[test]
fn paste_graph_fragment_rejects_immutable_help_source_target() {
    let mut session = RuntimeSession::default();
    load_sample_project(&mut session);
    let mut operation = paste_operation("1");
    operation.request.target.path = skenion_contracts::PatchPath::PackagePatchDefinition {
        package_id: "skenion.core".to_owned(),
        patch_id: "float-help".to_owned(),
        version: Some("0.37.0".to_owned()),
    };

    let response = session.apply_runtime_operation(operation);

    assert!(!response.ok);
    assert!(!response.applied);
    assert_eq!(
        response.issues[0].code,
        "paste.target.immutable-help-source"
    );
}

#[test]
fn paste_graph_fragment_converts_current_port_rates_for_lowered_execution_nodes() {
    let cases = [
        (
            json!({ "id": "event", "direction": "input", "type": "value.core.bang", "rate": "event", "triggerMode": "trigger" }),
            crate::DataFlow::Event,
            "value.core.bang",
            Some(crate::PortActivation::Trigger),
        ),
        (
            json!({ "id": "message", "direction": "input", "type": "value.core.message", "rate": "control", "triggerMode": "trigger" }),
            crate::DataFlow::Control,
            "value.core.message",
            Some(crate::PortActivation::Trigger),
        ),
        (
            json!({ "id": "audio", "direction": "output", "type": "value.core.float32", "rate": "audio" }),
            crate::DataFlow::Signal,
            "value.core.float32",
            None,
        ),
        (
            json!({ "id": "resource", "direction": "input", "type": "resource.buffer", "rate": "resource" }),
            crate::DataFlow::Resource,
            "resource.buffer",
            None,
        ),
        (
            json!({ "id": "io", "direction": "output", "type": "io.midi", "rate": "io" }),
            crate::DataFlow::Resource,
            "io.midi",
            None,
        ),
        (
            json!({ "id": "render", "direction": "input", "type": "value.core.float32", "rate": "render", "triggerMode": "passive" }),
            crate::DataFlow::Control,
            "value.core.float32",
            Some(crate::PortActivation::Latched),
        ),
        (
            json!({ "id": "gpu", "direction": "input", "type": "value.core.color", "rate": "gpu", "triggerMode": "latched" }),
            crate::DataFlow::Control,
            "value.core.color",
            Some(crate::PortActivation::Latched),
        ),
        (
            json!({ "id": "texture", "direction": "output", "type": "value.core.tensor", "rate": "gpu" }),
            crate::DataFlow::Resource,
            "value.core.tensor",
            None,
        ),
        (
            json!({ "id": "default", "direction": "input", "type": "value.core.message" }),
            crate::DataFlow::Control,
            "value.core.message",
            None,
        ),
    ];

    for (value, expected_flow, expected_kind, expected_activation) in cases {
        let port: PortSpecCurrent = serde_json::from_value(value).expect("port should parse");
        let lowered = lower_port_for_execution(&port);
        assert_eq!(lowered.data_type.flow, expected_flow);
        assert_eq!(lowered.data_type.data_kind, expected_kind);
        assert_eq!(lowered.activation, expected_activation);
    }
}

fn graph_patch(value: Value) -> GraphPatch {
    serde_json::from_value(value).expect("patch should parse")
}

fn patch_graph(response: &RuntimePatchResponse) -> &GraphDocumentCurrent {
    &response
        .snapshot
        .project
        .as_ref()
        .expect("patch response should include project")
        .graph
}

fn patch_view_state(response: &RuntimePatchResponse) -> &ViewState {
    &response
        .snapshot
        .project
        .as_ref()
        .expect("patch response should include project")
        .view_state
}

fn assert_graph_patch_rejected(response: &RuntimePatchResponse) {
    assert!(!response.ok);
    assert!(!response.applied);
    assert!(!response.conflict);
    assert_eq!(
        response.issues[0].code.as_deref(),
        Some("project.graph-patch-unsupported")
    );
}

fn latest_history_entry(response: &RuntimePatchResponse) -> Option<&super::RuntimeHistoryEntry> {
    response.history.entries.last()
}

fn graph_mutation(patch: GraphPatch) -> RuntimeMutationRequest {
    RuntimeMutationRequest {
        graph_patch: Some(patch),
        view_patch: None,
        actor_id: None,
        client_id: None,
        description: None,
    }
}

fn empty_runtime_mutation() -> RuntimeMutationRequest {
    RuntimeMutationRequest {
        graph_patch: None,
        view_patch: None,
        actor_id: None,
        client_id: None,
        description: None,
    }
}

fn described_runtime_mutation(description: &str) -> RuntimeMutationRequest {
    RuntimeMutationRequest {
        graph_patch: None,
        view_patch: None,
        actor_id: None,
        client_id: None,
        description: Some(description.to_owned()),
    }
}

fn set_value_patch(base_revision: &str, value: f64) -> GraphPatch {
    graph_patch(json!({
      "schema": "skenion.graph.patch",
      "schemaVersion": "0.1.0",
      "id": "set-value",
      "baseRevision": base_revision,
      "ops": [
        { "op": "setNodeParam", "nodeId": "value_1", "key": "value", "value": value }
      ]
    }))
}

fn paste_operation(base_revision: &str) -> RuntimeOperationEnvelope {
    current_fixture(
        json!({
          "schema": "skenion.runtime.operation",
          "schemaVersion": "0.1.0",
          "id": "op-paste",
          "kind": "pasteGraphFragment",
          "request": {
            "target": {
              "path": { "kind": "root" },
              "baseRevision": base_revision
            },
            "fragment": paste_fragment_json(),
            "options": {
              "idConflictPolicy": "remap"
            }
          },
          "attribution": {
            "clientId": "studio-test",
            "label": "Paste test fragment"
          }
        }),
        "paste operation should parse",
    )
}

fn paste_fragment_json() -> Value {
    let mut fragment = json!({
      "schema": "skenion.graph.fragment",
      "schemaVersion": "0.1.0",
      "nodes": [
        {
          "id": "value_1",
          "kind": "object.core.float",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": value_f32_ports_current_json()
        },
        {
          "id": "pasted_target",
          "kind": "object.core.float",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": value_f32_ports_current_json()
        }
      ],
      "edges": [
        {
          "id": "edge_value_to_pasted",
          "source": { "nodeId": "value_1", "portId": "value" },
          "target": { "nodeId": "pasted_target", "portId": "cold" }
        }
      ],
      "view": {
        "nodes": {
          "value_1": { "x": 10.0, "y": 20.0 },
          "pasted_target": { "x": 180.0, "y": 20.0 }
        }
      }
    });
    normalize_current_fixture_value(&mut fragment);
    fragment
}

fn graph_node_current(id: &str) -> crate::GraphNodeCurrent {
    current_fixture(
        value_node_current_json(id),
        "current 0.1 graph node should parse",
    )
}

fn value_node_current_json(id: &str) -> Value {
    current_core_node_json(
        id,
        "float",
        "float",
        json!({}),
        value_f32_ports_current_json(),
    )
}

fn patch_definition_current(id: &str) -> skenion_contracts::PatchDefinitionV01 {
    current_fixture(
        json!({
          "id": id,
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": format!("{id}-graph"),
            "revision": "1",
            "nodes": [value_node_current_json("patch_value")],
            "edges": []
          }
        }),
        "patch definition should parse",
    )
}

fn collaboration_change(value: Value) -> RuntimeCollaborationChange {
    current_fixture(value, "collaboration change should parse")
}

fn f32_value(value: f64) -> ControlValue {
    ControlValue::float(value)
}

fn control_request(
    node_id: &str,
    port_id: &str,
    value: ControlValue,
) -> RuntimeControlEventRequest {
    RuntimeControlEventRequest {
        node_id: node_id.to_owned(),
        port_id: port_id.to_owned(),
        message: ControlMessage::from_value(value),
    }
}

fn set_control_request(
    node_id: &str,
    port_id: &str,
    value: ControlValue,
) -> RuntimeControlEventRequest {
    RuntimeControlEventRequest {
        node_id: node_id.to_owned(),
        port_id: port_id.to_owned(),
        message: ControlMessage {
            key: "set".to_owned(),
            atoms: vec![value],
        },
    }
}

fn bang_control_request(node_id: &str, port_id: &str) -> RuntimeControlEventRequest {
    RuntimeControlEventRequest {
        node_id: node_id.to_owned(),
        port_id: port_id.to_owned(),
        message: ControlMessage::bang(),
    }
}

fn emitted_value(emission: &RuntimeControlEmission) -> Option<ControlValue> {
    emission.message.first_atom().cloned()
}

fn control_read(
    node_id: &str,
    target: RuntimeControlReadTarget,
    id: &str,
) -> RuntimeControlReadRequest {
    RuntimeControlReadRequest {
        node_id: node_id.to_owned(),
        target,
        id: id.to_owned(),
    }
}

fn duplicate_edge_patch() -> GraphPatch {
    graph_patch(json!({
      "schema": "skenion.graph.patch",
      "schemaVersion": "0.1.0",
      "id": "duplicate-edge",
      "baseRevision": "1",
      "ops": [
        {
          "op": "addEdge",
          "edge": {
            "from": { "node": "value_1", "port": "value" },
            "to": { "node": "target_1", "port": "in" }
          }
        }
      ]
    }))
}

fn missing_node_patch() -> GraphPatch {
    graph_patch(json!({
      "schema": "skenion.graph.patch",
      "schemaVersion": "0.1.0",
      "id": "missing-node",
      "baseRevision": "1",
      "ops": [
        { "op": "setNodeParam", "nodeId": "missing", "key": "value", "value": 1 }
      ]
    }))
}

fn incompatible_edge_patch() -> GraphPatch {
    graph_patch(json!({
      "schema": "skenion.graph.patch",
      "schemaVersion": "0.1.0",
      "id": "incompatible-edge",
      "baseRevision": "1",
      "ops": [
        {
          "op": "addEdge",
          "edge": {
            "from": { "node": "value_1", "port": "value" },
            "to": { "node": "target_1", "port": "value" }
          }
        }
      ]
    }))
}

fn missing_definition_node_patch() -> GraphPatch {
    graph_patch(json!({
      "schema": "skenion.graph.patch",
      "schemaVersion": "0.1.0",
      "id": "missing-definition-node",
      "baseRevision": "1",
      "ops": [
        {
          "op": "addNode",
          "node": {
            "id": "missing_kind_1",
            "kind": "missing.kind",
            "kindVersion": "0.1.0",
            "params": {},
            "ports": []
          }
        }
      ]
    }))
}

fn load_sample_project(session: &mut RuntimeSession) -> super::RuntimeSessionResponse {
    session.load_project_current(sample_project_current())
}

fn sample_project_current() -> ProjectRequestCurrent {
    current_fixture(
        json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "minimal-value",
            "revision": "1",
            "nodes": [
              {
                "id": "value_1",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_current_json()
              },
              {
                "id": "target_1",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_current_json()
              }
            ],
            "edges": [
              {
                "id": "edge_value_target",
                "source": { "nodeId": "value_1", "portId": "value" },
                "target": { "nodeId": "target_1", "portId": "cold" },
                "resolvedType": "value.core.float32"
              }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.float",
              "version": "0.1.0",
              "displayName": "Float",
              "category": "Typed Controls",
              "ports": value_f32_ports_current_json(),
              "execution": { "model": "control" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["value.core.float32.v0.1"]
            }
          ],
          "viewState": {
            "schema": "skenion.view-state",
            "schemaVersion": "0.1.0",
            "canvas": {
              "nodes": {
                "value_1": { "x": 96.0, "y": 96.0 },
                "target_1": { "x": 260.0, "y": 96.0 }
              }
            }
          }
        }),
        "current 0.1 sample project should parse",
    )
}

fn binding_project_current() -> ProjectRequestCurrent {
    current_fixture(
        json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "minimal-binding",
            "revision": "1",
            "nodes": [
              {
                "id": "value_1",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_current_json()
              },
              {
                "id": "target_1",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_current_json()
              }
            ],
            "edges": [
              {
                "id": "edge_value_target",
                "source": { "nodeId": "value_1", "portId": "value" },
                "target": { "nodeId": "target_1", "portId": "cold" },
                "resolvedType": "value.core.float32"
              }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.float",
              "version": "0.1.0",
              "displayName": "Float",
              "category": "Typed Controls",
              "ports": value_f32_ports_current_json(),
              "execution": { "model": "control" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["value.core.float32.v0.1"]
            }
          ],
          "viewState": {
            "schema": "skenion.view-state",
            "schemaVersion": "0.1.0",
            "canvas": {
              "nodes": {
                "value_1": { "x": 96.0, "y": 96.0 },
                "target_1": { "x": 260.0, "y": 96.0 }
              }
            }
          }
        }),
        "binding sample project should parse",
    )
}

fn unresolved_project_current() -> ProjectRequestCurrent {
    let mut request = sample_project_current();
    request.graph.nodes.push(current_fixture(
        unresolved_node_json("unresolved_1", "user.manipulator"),
        "unresolved current 0.1 node should parse",
    ));
    request
}

fn object_routing_project_current() -> ProjectRequestCurrent {
    let mut request = sample_project_current();
    request.graph.nodes[0]
        .params
        .insert("sendName".to_owned(), json!("speed"));
    request
}

fn debug_sink_project_current() -> ProjectRequestCurrent {
    let mut request = sample_project_current();
    request.graph.nodes.push(current_fixture(
        json!({
            "id": "debug_1",
            "kind": "debug.sink",
            "kindVersion": "0.1.0",
            "params": {},
            "ports": []
        }),
        "debug current 0.1 node should parse",
    ));
    request.nodes.push(
        serde_json::from_value(json!({
            "schema": "skenion.node.definition",
            "schemaVersion": "0.1.0",
            "id": "debug.sink",
            "version": "0.1.0",
            "displayName": "Debug Sink",
            "category": "Debug",
            "ports": [],
            "execution": { "model": "control" },
            "state": { "persistent": false },
            "permissions": [],
            "capabilities": []
        }))
        .expect("debug current 0.1 definition should parse"),
    );
    request
}

fn value_f32_ports_current_json() -> Value {
    json!([
      {
        "id": "in",
        "direction": "input",
        "label": "In",
        "type": "value.core.message",
        "rate": "control",
        "required": false,
        "triggerMode": "trigger",
        "accepts": [
          "value.core.float32",
          "value.core.int32",
          "value.core.uint32",
          "value.core.bool",
          "value.core.bang"
        ],
        "messageKeys": {
          "accepted": ["bang", "set", "float", "int", "uint", "bool"],
          "silent": ["set"],
          "trigger": ["bang", "float", "int", "uint", "bool"],
          "store": ["set", "float", "int", "uint", "bool"],
          "emit": ["bang", "float", "int", "uint", "bool"]
        }
      },
      {
        "id": "cold",
        "direction": "input",
        "label": "Cold",
        "type": "value.core.float32",
        "rate": "control",
        "required": false,
        "triggerMode": "passive"
      },
      {
        "id": "value",
        "direction": "output",
        "label": "Value",
        "type": "value.core.float32",
        "rate": "control"
      }
    ])
}

fn unresolved_node_json(id: &str, object_spec: &str) -> Value {
    current_unresolved_node_json(id, object_spec)
}
