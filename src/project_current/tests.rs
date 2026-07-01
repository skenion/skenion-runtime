use serde_json::{Value, json};

use super::*;
use crate::{
    FanOutPolicyCurrent, FeedbackBoundaryCurrent, IssueSeverity, PortDirectionCurrent,
    PortRateCurrent, PortSpecCurrent,
};

fn core_impl(object_id: &str) -> crate::ObjectImplementationRefCurrent {
    crate::ObjectImplementationRefCurrent {
        provider: crate::ObjectProviderRefCurrent::Core,
        object_id: object_id.to_owned(),
        interface_digest: None,
    }
}

fn current_core_node_json(id: &str, object_id: &str, params: Value, ports: Value) -> Value {
    json!({
      "id": id,
      "implementation": {
        "provider": { "kind": "core" },
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

fn current_provider_node_json(id: &str, object_id: &str, params: Value, ports: Value) -> Value {
    json!({
      "id": id,
      "implementation": {
        "provider": {
          "kind": "package",
          "packageId": "test/current-fixtures",
          "version": CURRENT_SCHEMA_VERSION
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
                *value = if kind == "p" {
                    let mut params = params;
                    if let Some(patch_ref) = object_spec
                        .as_deref()
                        .and_then(|spec| spec.strip_prefix("p "))
                        .filter(|patch_ref| !patch_ref.is_empty())
                        && let Some(map) = params.as_object_mut()
                    {
                        map.entry("patchRef".to_owned())
                            .or_insert_with(|| json!(patch_ref));
                    }
                    current_core_node_json(&id, "subpatch", params, ports)
                } else if let Some(object_id) = kind.strip_prefix("object.core.") {
                    let mut node = current_core_node_json(&id, object_id, params, ports);
                    if let Some(object_spec) = object_spec.as_ref()
                        && let Some(map) = node.as_object_mut()
                    {
                        map.insert("objectSpec".to_owned(), json!(object_spec));
                    }
                    node
                } else {
                    let mut node = current_provider_node_json(&id, &kind, params, ports);
                    if let Some(object_spec) = object_spec.as_ref()
                        && let Some(map) = node.as_object_mut()
                    {
                        map.insert("objectSpec".to_owned(), json!(object_spec));
                    }
                    node
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

fn graph(value: Value) -> GraphDocumentCurrent {
    current_fixture(value, "graph should parse")
}

fn definition(value: Value) -> NodeDefinitionCurrent {
    serde_json::from_value(value).expect("definition should parse")
}

fn clear_definition() -> NodeDefinitionCurrent {
    definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.render.clear-color",
      "version": "0.1.0",
      "displayName": "Clear Color",
      "category": "Render",
      "ports": [
        { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
      ],
      "execution": { "model": "gpu_pass", "clock": "frame" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn output_definition() -> NodeDefinitionCurrent {
    definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.render.output",
      "version": "0.1.0",
      "displayName": "Render Output",
      "category": "Render",
      "ports": [
        { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true }
      ],
      "execution": { "model": "gpu_pass", "clock": "frame" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn float_definition() -> NodeDefinitionCurrent {
    definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.float",
      "version": "0.1.0",
      "displayName": "Float",
      "category": "Core",
      "ports": [
        {
          "id": "in",
          "direction": "input",
          "type": "value.core.message",
          "rate": "control",
          "triggerMode": "trigger",
          "messageKeys": {
            "accepted": ["set", "bang"],
            "store": ["set"],
            "trigger": ["bang"]
          }
        },
        { "id": "cold", "direction": "input", "type": "value.core.float32", "rate": "control" },
        { "id": "value", "direction": "output", "type": "value.core.float32", "rate": "control" }
      ],
      "execution": { "model": "control" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn float_pair_graph() -> GraphDocumentCurrent {
    graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "float-message-selector",
      "revision": "1",
      "nodes": [
        {
          "id": "value_1",
          "kind": "object.core.float",
          "kindVersion": "0.1.0",
          "objectSpec": "float",
          "params": { "value": 0.5 },
          "ports": [
            {
              "id": "in",
              "direction": "input",
              "type": "value.core.message",
              "rate": "control",
              "triggerMode": "trigger",
              "messageKeys": {
                "accepted": ["set", "bang"],
                "store": ["set"],
                "trigger": ["bang"]
              }
            },
            { "id": "cold", "direction": "input", "type": "value.core.float32", "rate": "control" },
            { "id": "value", "direction": "output", "type": "value.core.float32", "rate": "control" }
          ]
        },
        {
          "id": "target_1",
          "kind": "object.core.float",
          "kindVersion": "0.1.0",
          "objectSpec": "float",
          "params": { "value": 0.0 },
          "ports": [
            {
              "id": "in",
              "direction": "input",
              "type": "value.core.message",
              "rate": "control",
              "triggerMode": "trigger",
              "messageKeys": {
                "accepted": ["set", "bang"],
                "store": ["set"],
                "trigger": ["bang"]
              }
            },
            { "id": "cold", "direction": "input", "type": "value.core.float32", "rate": "control" },
            { "id": "value", "direction": "output", "type": "value.core.float32", "rate": "control" }
          ]
        }
      ],
      "edges": [
        {
          "id": "edge_value_target",
          "source": { "nodeId": "value_1", "portId": "value" },
          "target": { "nodeId": "target_1", "portId": "in" },
          "resolvedType": "value.core.float32"
        }
      ]
    }))
}

fn pass_definition() -> NodeDefinitionCurrent {
    definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "test.pass",
      "version": "0.1.0",
      "displayName": "Pass",
      "category": "Test",
      "ports": [
        { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true },
        { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
      ],
      "execution": { "model": "gpu_pass", "clock": "frame" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn behavior_definition(id: &str) -> NodeDefinitionCurrent {
    definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": id,
      "version": "0.1.0",
      "displayName": id,
      "category": "Core",
      "ports": [],
      "execution": { "model": "control" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn object_spec_ports_json(object_spec: &str) -> Value {
    let resolution = crate::object_spec::resolve_object_spec_v01(object_spec);
    assert!(
        resolution.ok(),
        "{object_spec} should resolve without issues: {:?}",
        resolution.issues
    );
    let definition = crate::object_spec::object_spec_node_definition_v01(&resolution)
        .expect("resolved object spec should project to a node definition");
    serde_json::to_value(definition.ports).expect("object spec ports should serialize")
}

fn render_graph() -> GraphDocumentCurrent {
    graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "render",
      "revision": "1",
      "nodes": [
        {
          "id": "clear",
          "kind": "object.core.render.clear-color",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
          ]
        },
        {
          "id": "output",
          "kind": "object.core.render.output",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true }
          ]
        }
      ],
      "edges": [
        {
          "id": "edge_clear_output",
          "source": { "nodeId": "clear", "portId": "out" },
          "target": { "nodeId": "output", "portId": "in" },
          "resolvedType": "value.core.tensor"
        }
      ]
    }))
}

fn identity_patch() -> PatchDefinitionCurrent {
    current_fixture(
        json!({
          "id": "identity",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "identity-graph",
            "revision": "1",
            "nodes": [
              {
                "id": "patch_in",
                "kind": "object.core.inlet",
                "kindVersion": "0.1.0",
                "params": { "portId": "in", "label": "Input" },
                "ports": [
                  { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render", "description": "Frame entering the patch" }
                ]
              },
              {
                "id": "pass",
                "kind": "test.pass",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": [
                  { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true },
                  { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
                ]
              },
              {
                "id": "patch_out",
                "kind": "object.core.outlet",
                "kindVersion": "0.1.0",
                "params": { "portId": "out", "label": "Output" },
                "ports": [
                  { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true, "description": "Frame leaving the patch" }
                ]
              }
            ],
            "edges": [
              {
                "id": "edge_in_pass",
                "source": { "nodeId": "patch_in", "portId": "out" },
                "target": { "nodeId": "pass", "portId": "in" },
                "resolvedType": "value.core.tensor"
              },
              {
                "id": "edge_pass_out",
                "source": { "nodeId": "pass", "portId": "out" },
                "target": { "nodeId": "patch_out", "portId": "in" },
                "resolvedType": "value.core.tensor"
              }
            ]
          }
        }),
        "patch definition should parse",
    )
}

fn subpatch_graph() -> GraphDocumentCurrent {
    graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "render-subpatch",
      "revision": "1",
      "nodes": [
        {
          "id": "clear",
          "kind": "object.core.render.clear-color",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
          ]
        },
        {
          "id": "fx",
          "kind": "object.core.subpatch",
          "kindVersion": "0.1.0",
          "params": { "patchRef": "identity" },
          "ports": [
            { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true },
            { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
          ]
        },
        {
          "id": "output",
          "kind": "object.core.render.output",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true }
          ]
        }
      ],
      "edges": [
        {
          "id": "edge_clear_fx",
          "source": { "nodeId": "clear", "portId": "out" },
          "target": { "nodeId": "fx", "portId": "in" },
          "resolvedType": "value.core.tensor"
        },
        {
          "id": "edge_fx_output",
          "source": { "nodeId": "fx", "portId": "out" },
          "target": { "nodeId": "output", "portId": "in" },
          "resolvedType": "value.core.tensor"
        }
      ]
    }))
}

fn project_document() -> ProjectDocumentCurrent {
    current_fixture(
        json!({
          "schema": "skenion.project",
          "schemaVersion": "0.1.0",
          "id": "render-project",
          "documentId": "30000000-0000-0000-0000-000000000003",
          "revision": "1",
          "graph": subpatch_graph(),
          "viewState": {
            "schema": "skenion.view-state",
            "schemaVersion": "0.1.0",
            "canvas": { "nodes": {} }
          },
          "patchLibrary": [identity_patch()]
        }),
        "project document should parse",
    )
}

#[test]
fn validates_and_builds_current_plan_metadata() {
    let graph = render_graph();
    let nodes = vec![clear_definition(), output_definition()];
    let (warnings, analysis) =
        validate_project_current(&graph, &nodes).expect("project should validate");
    assert!(warnings.is_empty());
    assert!(analysis.cycles.is_empty());

    let (plan, issues) = build_execution_plan_current(&graph, &nodes).expect("plan should build");
    assert!(issues.is_empty());
    assert_eq!(plan.graph_id, "render");
    assert_eq!(plan.nodes.len(), 2);
    assert_eq!(plan.groups.len(), 1);
    let metadata = plan.edges[0]
        .metadata
        .as_ref()
        .expect("metadata should exist");
    assert_eq!(metadata.resolved_type.as_deref(), Some("value.core.tensor"));
    assert_eq!(metadata.merge_policy.as_deref(), Some("forbid"));
    assert_eq!(metadata.fan_out_policy.as_deref(), Some("allow"));
    assert_eq!(metadata.cycle_classification, None);
}

#[test]
fn records_merge_order_feedback_and_risky_warnings() {
    let mut graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "feedback",
      "revision": "1",
      "nodes": [
        {
          "id": "node",
          "kind": "object.core.render.feedback-composite",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "previous", "direction": "input", "type": "value.core.tensor", "rate": "render" },
            { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render", "fanOutPolicy": "copy" }
          ]
        }
      ],
      "edges": [
        {
          "id": "edge_feedback",
          "source": { "nodeId": "node", "portId": "out" },
          "target": { "nodeId": "node", "portId": "previous" },
          "order": 3,
          "feedback": { "enabled": true, "boundary": "render-frame", "intentional": true }
        }
      ]
    }));
    let definition = definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.render.feedback-composite",
      "version": "0.1.0",
      "displayName": "Feedback",
      "category": "Render",
      "ports": [
        { "id": "previous", "direction": "input", "type": "value.core.tensor", "rate": "render" },
        { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render", "fanOutPolicy": "copy" }
      ],
      "execution": { "model": "gpu_pass", "clock": "frame" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }));

    let (plan, issues) = build_execution_plan_current(&graph, std::slice::from_ref(&definition))
        .expect("feedback should plan");
    assert!(issues.is_empty());
    let metadata = plan.edges[0].metadata.as_ref().unwrap();
    assert_eq!(metadata.order, Some(3));
    assert_eq!(metadata.fan_out_policy.as_deref(), Some("copy"));
    assert_eq!(
        metadata.cycle_classification.as_deref(),
        Some("valid-feedback")
    );
    assert_eq!(
        metadata.feedback.as_ref().unwrap().boundary,
        FeedbackBoundaryCurrent::RenderFrame
    );

    graph.edges[0].feedback.as_mut().unwrap().boundary = FeedbackBoundaryCurrent::SameTurn;
    let (_plan, issues) =
        build_execution_plan_current(&graph, &[definition]).expect("risky feedback should plan");
    assert!(issues.iter().any(|issue| {
        issue.severity == IssueSeverity::Warning
            && issue.code.as_deref() == Some("graph.risky-feedback")
    }));
}

#[test]
fn project_document_conversions_default_runtime_fields() {
    let document = project_document();

    let request: ProjectRequestCurrent = document.clone().into();
    assert_eq!(request.graph.id, "render-subpatch");
    assert!(request.nodes.is_empty());
    assert_eq!(request.patch_library[0].id, "identity");

    let run_request: RunProjectRequestCurrent = document.into();
    assert_eq!(run_request.graph.id, "render-subpatch");
    assert!(run_request.nodes.is_empty());
    assert_eq!(run_request.patch_library[0].id, "identity");
    assert_eq!(run_request.frames, None);
}

#[test]
fn expands_subpatches_before_current_validation_and_planning() {
    let request = ProjectRequestCurrent {
        document: None,
        graph: subpatch_graph(),
        nodes: vec![clear_definition(), output_definition(), pass_definition()],
        patch_library: vec![identity_patch()],
        view_state: None,
    };

    let expanded = expand_project_graph_current(&request.graph, &request.patch_library)
        .expect("subpatch graph should expand");
    let node_ids = expanded
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(node_ids, vec!["clear", "fx::pass", "output"]);
    assert!(expanded.edges.iter().any(|edge| {
        edge.source.node_id == "clear"
            && edge.source.port_id == "out"
            && edge.target.node_id == "fx::pass"
            && edge.target.port_id == "in"
    }));
    assert!(expanded.edges.iter().any(|edge| {
        edge.source.node_id == "fx::pass"
            && edge.source.port_id == "out"
            && edge.target.node_id == "output"
            && edge.target.port_id == "in"
    }));

    let (issues, _) =
        validate_project_request_current(&request).expect("expanded project should validate");
    assert!(issues.is_empty());
    let (plan, issues) =
        build_execution_plan_request_current(&request).expect("expanded project should plan");
    assert!(issues.is_empty());
    assert_eq!(
        plan.nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>(),
        vec!["clear", "fx::pass", "output"]
    );
}

#[test]
fn current_plan_sorts_expanded_nodes_by_dependency_order() {
    let mut graph = subpatch_graph();
    graph.nodes.reverse();
    let request = ProjectRequestCurrent {
        document: None,
        graph,
        nodes: vec![clear_definition(), output_definition(), pass_definition()],
        patch_library: vec![identity_patch()],
        view_state: None,
    };

    let expanded = expand_project_graph_current(&request.graph, &request.patch_library)
        .expect("subpatch graph should expand");
    assert_eq!(
        expanded
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["output", "fx::pass", "clear"]
    );

    let (plan, issues) =
        build_execution_plan_request_current(&request).expect("expanded project should plan");

    assert!(issues.is_empty());
    assert_eq!(
        plan.nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>(),
        vec!["clear", "fx::pass", "output"]
    );
    assert_eq!(
        plan.nodes.iter().map(|node| node.order).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[test]
fn contracts_boundary_edges_and_filters_boundary_only_edges() {
    let base_edge = render_graph().edges[0].clone();
    let endpoint = EdgeEndpointCurrent {
        node_id: "same".to_owned(),
        port_id: "out".to_owned(),
    };
    let self_loop_edges = vec![
        ExpansionEdge {
            edge: base_edge.clone(),
            source: ExpansionEndpoint::Node(endpoint.clone()),
            target: ExpansionEndpoint::Boundary("pin".to_owned()),
        },
        ExpansionEdge {
            edge: base_edge.clone(),
            source: ExpansionEndpoint::Boundary("pin".to_owned()),
            target: ExpansionEndpoint::Node(endpoint.clone()),
        },
    ];

    let contracted = contract_boundary_edges(
        self_loop_edges,
        std::collections::HashSet::from(["pin".to_owned()]),
    );
    assert!(contracted.is_empty());

    let mut source_edge = base_edge.clone();
    source_edge.id = "source_edge".to_owned();
    source_edge.resolved_type = Some("value.core.tensor".to_owned());
    let mut target_edge = base_edge.clone();
    target_edge.id = "target_edge".to_owned();
    target_edge.resolved_type = None;
    let merged = contract_boundary_edges(
        vec![
            ExpansionEdge {
                edge: source_edge,
                source: ExpansionEndpoint::Node(EdgeEndpointCurrent {
                    node_id: "source".to_owned(),
                    port_id: "out".to_owned(),
                }),
                target: ExpansionEndpoint::Boundary("fx::@inlet::in".to_owned()),
            },
            ExpansionEdge {
                edge: target_edge,
                source: ExpansionEndpoint::Boundary("fx::@inlet::in".to_owned()),
                target: ExpansionEndpoint::Node(EdgeEndpointCurrent {
                    node_id: "target".to_owned(),
                    port_id: "in".to_owned(),
                }),
            },
        ],
        std::collections::HashSet::from(["fx::@inlet::in".to_owned()]),
    );
    assert_eq!(merged.len(), 1);
    assert_eq!(
        merged[0].resolved_type.as_deref(),
        Some("value.core.tensor")
    );
    assert!(merged[0].id.contains("fx___inlet__in"));

    assert!(
        expansion_edge_to_real_edge(ExpansionEdge {
            edge: base_edge.clone(),
            source: ExpansionEndpoint::Boundary("pin".to_owned()),
            target: ExpansionEndpoint::Node(endpoint.clone()),
        })
        .is_none()
    );
    assert!(
        expansion_edge_to_real_edge(ExpansionEdge {
            edge: base_edge,
            source: ExpansionEndpoint::Node(endpoint),
            target: ExpansionEndpoint::Boundary("pin".to_owned()),
        })
        .is_none()
    );
}

#[test]
fn reports_missing_ref_depth_and_duplicate_patch_issues() {
    let duplicate = ProjectRequestCurrent {
        document: None,
        graph: render_graph(),
        nodes: vec![clear_definition(), output_definition()],
        patch_library: vec![identity_patch(), identity_patch()],
        view_state: None,
    };
    let duplicate_issues =
        validate_project_request_current(&duplicate).expect_err("duplicate patch ids should fail");
    assert_eq!(
        duplicate_issues[0].code.as_deref(),
        Some("subpatch.duplicate-patch-id")
    );

    let missing_ref = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "missing-ref",
      "revision": "1",
      "nodes": [
        {
          "id": "fx",
          "kind": "object.core.subpatch",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": []
        }
      ],
      "edges": []
    }));
    let missing_ref_issues =
        expand_project_graph_current(&missing_ref, &[]).expect_err("missing ref should fail");
    assert_eq!(
        missing_ref_issues[0].code.as_deref(),
        Some("subpatch.missing-ref")
    );
    assert_eq!(
        missing_ref_issues[0].details.as_ref().unwrap()["patchRef"],
        Value::Null
    );

    let mut patch_library = Vec::new();
    for index in 0..=15 {
        patch_library.push(current_fixture(
            json!({
              "id": format!("p{index}"),
              "revision": "1",
              "graph": {
                "schema": "skenion.graph",
                "schemaVersion": "0.1.0",
                "id": format!("p{index}-graph"),
                "revision": "1",
                "nodes": [
                  {
                    "id": "next",
                    "kind": "object.core.subpatch",
                    "kindVersion": "0.1.0",
                    "params": { "patchRef": format!("p{}", index + 1) },
                    "ports": []
                  }
                ],
                "edges": []
              }
            }),
            "patch should parse",
        ));
    }
    let depth_root = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "depth-root",
      "revision": "1",
      "nodes": [
        {
          "id": "root",
          "kind": "object.core.subpatch",
          "kindVersion": "0.1.0",
          "params": { "patchRef": "p0" },
          "ports": []
        }
      ],
      "edges": []
    }));
    let depth_issues =
        expand_project_graph_current(&depth_root, &patch_library).expect_err("depth should fail");
    assert_eq!(
        depth_issues[0].code.as_deref(),
        Some("subpatch.depth-exceeded")
    );
    assert_eq!(depth_issues[0].details.as_ref().unwrap()["depth"], 17);
}

#[test]
fn parses_subpatch_aliases_and_reports_missing_boundaries() {
    assert_eq!(
        parse_subpatch_object_spec("p identity").as_deref(),
        Some("identity")
    );
    assert_eq!(
        parse_subpatch_object_spec("object.core.subpatch identity").as_deref(),
        Some("identity")
    );
    assert_eq!(parse_subpatch_object_spec("object identity"), None);
    assert_eq!(namespace_prefix(""), "");

    let params = serde_json::Map::from_iter([
        ("patchId".to_owned(), json!(42)),
        ("empty".to_owned(), json!("")),
        ("enabled".to_owned(), json!(true)),
    ]);
    assert_eq!(string_param(&params, "patchId").as_deref(), Some("42"));
    assert_eq!(string_param(&params, "empty"), None);
    assert_eq!(string_param(&params, "enabled"), None);

    let fallback_boundary = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "fallback-boundary",
      "revision": "1",
      "nodes": [
        {
          "id": "plain_inlet",
          "kind": "object.core.inlet",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": []
        }
      ],
      "edges": []
    }));
    assert_eq!(boundary_key(&fallback_boundary.nodes[0]), "plain_inlet");

    let duplicate_inlet_patch: PatchDefinitionCurrent = current_fixture(
        json!({
          "id": "alias-patch",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "alias-patch-graph",
            "revision": "1",
            "nodes": [
              {
                "id": "in_a",
                "kind": "object.core.inlet",
                "kindVersion": "0.1.0",
                "params": { "portId": "in_a", "label": "shared" },
                "ports": [
                  { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
                ]
              },
              {
                "id": "in_b",
                "kind": "object.core.inlet",
                "kindVersion": "0.1.0",
                "params": { "portId": "in_b", "label": "shared" },
                "ports": [
                  { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
                ]
              }
            ],
            "edges": []
          }
        }),
        "patch should parse",
    );
    let mut boundary_pins = std::collections::HashSet::new();
    let mut aliases = std::collections::HashMap::new();
    let first_pin = register_boundary_node(
        &duplicate_inlet_patch.graph.nodes[0],
        "fx",
        BoundaryKind::Inlet,
        &mut boundary_pins,
        &mut aliases,
    );
    let second_pin = register_boundary_node(
        &duplicate_inlet_patch.graph.nodes[0],
        "fx",
        BoundaryKind::Inlet,
        &mut boundary_pins,
        &mut aliases,
    );
    assert_eq!(first_pin, second_pin);

    let root = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "alias-root",
      "revision": "1",
      "nodes": [
        {
          "id": "clear",
          "kind": "object.core.render.clear-color",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
          ]
        },
        {
          "id": "fx",
          "kind": "p",
          "kindVersion": "0.1.0",
          "objectSpec": "p alias-patch",
          "params": {},
          "ports": [
            { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render" },
            { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
          ]
        },
        {
          "id": "output",
          "kind": "object.core.render.output",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render" }
          ]
        }
      ],
      "edges": [
        {
          "id": "edge_clear_fx",
          "source": { "nodeId": "clear", "portId": "out" },
          "target": { "nodeId": "fx", "portId": "shared" }
        },
        {
          "id": "edge_fx_output",
          "source": { "nodeId": "fx", "portId": "out" },
          "target": { "nodeId": "output", "portId": "in" }
        }
      ]
    }));
    let issues =
        expand_project_graph_current(&root, &[duplicate_inlet_patch]).expect_err("boundaries fail");
    let codes = issues
        .iter()
        .map(|issue| issue.code.as_deref())
        .collect::<Vec<_>>();
    assert!(codes.contains(&Some("subpatch.missing-inlet")));
    assert!(codes.contains(&Some("subpatch.missing-outlet")));
}

#[test]
fn reports_missing_recursive_and_invalid_patch_library_issues() {
    let missing = ProjectRequestCurrent {
        document: None,
        graph: subpatch_graph(),
        nodes: vec![clear_definition(), output_definition(), pass_definition()],
        patch_library: Vec::new(),
        view_state: None,
    };
    let missing_issues =
        validate_project_request_current(&missing).expect_err("missing patch should fail");
    assert_eq!(
        missing_issues[0].code.as_deref(),
        Some("subpatch.missing-patch")
    );

    let recursive_patch: PatchDefinitionCurrent = current_fixture(
        json!({
          "id": "recursive",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "recursive-graph",
            "revision": "1",
            "nodes": [
              {
                "id": "self",
                "kind": "object.core.subpatch",
                "kindVersion": "0.1.0",
                "params": { "patchRef": "recursive" },
                "ports": []
              }
            ],
            "edges": []
          }
        }),
        "recursive patch should parse",
    );
    let recursive = ProjectRequestCurrent {
        document: None,
        graph: graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "recursive-root",
          "revision": "1",
          "nodes": [
            {
              "id": "root",
              "kind": "object.core.subpatch",
              "kindVersion": "0.1.0",
              "params": { "patchRef": "recursive" },
              "ports": []
            }
          ],
          "edges": []
        })),
        nodes: Vec::new(),
        patch_library: vec![recursive_patch],
        view_state: None,
    };
    let recursive_issues =
        validate_project_request_current(&recursive).expect_err("recursive patch should fail");
    assert_eq!(
        recursive_issues[0].code.as_deref(),
        Some("subpatch.recursion")
    );

    let mut duplicate_boundary = identity_patch();
    duplicate_boundary.graph.nodes[2].params["portId"] = json!("in");
    let invalid = ProjectRequestCurrent {
        document: None,
        graph: render_graph(),
        nodes: vec![clear_definition(), output_definition()],
        patch_library: vec![duplicate_boundary],
        view_state: None,
    };
    let invalid_issues =
        validate_project_request_current(&invalid).expect_err("invalid patch should fail");
    assert_eq!(
        invalid_issues[0].code.as_deref(),
        Some("subpatch.invalid-patch-definition")
    );
}

fn assert_patch_graph_schema_issue(
    issues: &[RuntimeIssue],
    expected_code: &str,
    expected_received_schema_version: &str,
) {
    let issue = issues
        .iter()
        .find(|issue| issue.code.as_deref() == Some(expected_code))
        .unwrap_or_else(|| panic!("missing {expected_code} issue: {issues:#?}"));
    let details = issue
        .details
        .as_ref()
        .expect("schema issue should include details");

    assert_eq!(details["surface"], "graph");
    assert_eq!(details["patchId"], "identity");
    assert_eq!(details["expectedSchemaVersion"], "0.1.0");
    assert_eq!(
        details["receivedSchemaVersion"],
        expected_received_schema_version
    );
    assert!(
        issues.iter().all(|issue| {
            issue.code.as_deref() != Some("subpatch.invalid-patch-definition")
                || !issue.message.contains("schemaVersion")
        }),
        "patch graph schemaVersion should not also be reported as generic patch contract failure: {issues:#?}"
    );
}

#[test]
fn direct_requests_report_structured_patch_graph_schema_versions() {
    let schema_version = "9.9.9";
    let expected_code = "project.unsupported-schema-version";
    let mut patch = identity_patch();
    patch.graph.schema_version = schema_version.to_owned();
    let request = ProjectRequestCurrent {
        document: None,
        graph: render_graph(),
        nodes: vec![clear_definition(), output_definition(), pass_definition()],
        patch_library: vec![patch],
        view_state: None,
    };

    let validation_issues = validate_project_request_current(&request)
        .expect_err("patch graph schema mismatch should fail request validation");
    assert_patch_graph_schema_issue(&validation_issues, expected_code, schema_version);

    let planning_issues = build_execution_plan_request_current(&request)
        .expect_err("patch graph schema mismatch should fail request planning");
    assert_patch_graph_schema_issue(&planning_issues, expected_code, schema_version);
}

#[test]
fn rejects_payload_identity_node_kinds_and_definition_ids() {
    for payload_identity in [
        "object.core.bool",
        "object.core.string",
        "bool",
        "string",
        "value.number",
        "value.core.message",
        "value.core.bang",
        "value.core.string",
        "value.core.tensor",
    ] {
        let mut graph = render_graph();
        graph.nodes[0].implementation = Some(core_impl(payload_identity));
        graph.nodes[0].ports.clear();
        let graph_result =
            validate_project_current(&graph, &[clear_definition(), output_definition()])
                .expect_err("payload identity graph node kind should fail");
        assert!(
            graph_result.iter().any(|issue| {
                issue.code.as_deref() == Some("graph.payload-node-kind")
                    && issue.details.as_ref().unwrap()["objectId"] == payload_identity
            }),
            "{payload_identity}: {graph_result:#?}"
        );

        let mut definition = clear_definition();
        definition.id = payload_identity.to_owned();
        let definition_result =
            validate_project_current(&render_graph(), &[definition, output_definition()])
                .expect_err("payload identity definition id should fail");
        assert!(
            definition_result.iter().any(|issue| {
                issue.code.as_deref() == Some("node-definition.payload-identity-id")
                    && issue.details.as_ref().unwrap()["nodeDefinitionId"] == payload_identity
            }),
            "{payload_identity}: {definition_result:#?}"
        );
    }
}

#[test]
fn accepts_behavior_object_identities_that_still_exist() {
    let behavior_ids = [
        "object.core.float",
        "object.core.int",
        "object.core.bang",
        "object.core.message",
    ];
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "behavior-identities",
      "revision": "1",
      "nodes": behavior_ids
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let object_spec = kind.strip_prefix("object.core.").unwrap_or(kind);
            json!({
              "id": format!("node_{index}"),
              "kind": kind,
              "kindVersion": "0.1.0",
              "objectSpec": object_spec,
              "params": {},
              "ports": object_spec_ports_json(object_spec)
            })
        })
        .collect::<Vec<_>>(),
      "edges": []
    }));
    let definitions = behavior_ids
        .iter()
        .map(|id| {
            definition(json!({
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": id,
              "version": "0.1.0",
              "displayName": id,
              "category": "Core",
              "ports": [],
              "execution": { "model": "control" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }))
        })
        .collect::<Vec<_>>();

    let (issues, _) =
        validate_project_current(&graph, &definitions).expect("behavior ids should validate");

    assert!(issues.is_empty());
}

#[test]
fn validates_runtime_owned_object_spec_resolution() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "object-spec",
      "revision": "1",
      "nodes": [
        {
          "id": "add",
          "kind": "object.core.operator.add",
          "kindVersion": "0.1.0",
          "objectSpec": "+ 2",
          "params": {},
          "ports": object_spec_ports_json("+ 2")
        }
      ],
      "edges": []
    }));

    validate_project_current(&graph, &[behavior_definition("object.core.operator.add")])
        .expect("matching Runtime object spec should validate");

    let mut invalid_arg = graph.clone();
    invalid_arg.nodes[0].object_spec = Some("+ true".to_owned());
    let invalid_arg_result = validate_project_current(
        &invalid_arg,
        &[behavior_definition("object.core.operator.add")],
    )
    .expect_err("invalid Runtime object-spec args should fail");
    assert!(invalid_arg_result.iter().any(|issue| {
        issue.code.as_deref() == Some("object-spec.invalid-arg-type")
            && issue.details.as_ref().unwrap()["objectSpec"] == "+ true"
    }));

    let mut mismatch = graph.clone();
    mismatch.nodes[0].implementation = Some(core_impl("object.core.operator.sub"));
    let mismatch_result = validate_project_current(
        &mismatch,
        &[behavior_definition("object.core.operator.sub")],
    )
    .expect_err("resolved object implementation mismatch should fail");
    assert!(mismatch_result.iter().any(|issue| {
        issue.code.as_deref() == Some("object-spec.implementation-mismatch")
            && issue.details.as_ref().unwrap()["resolvedImplementation"]["objectId"]
                == "operator.add"
            && issue.details.as_ref().unwrap()["nodeImplementation"]["objectId"]
                == "object.core.operator.sub"
    }));

    let mut payload = graph.clone();
    payload.nodes[0].implementation = Some(core_impl("object.core.float"));
    payload.nodes[0].object_spec = Some("value.core.float32".to_owned());
    let payload_result =
        validate_project_current(&payload, &[behavior_definition("object.core.float")])
            .expect_err("payload identity object spec should fail");
    assert!(payload_result.iter().any(|issue| {
        issue.code.as_deref() == Some("object-spec.payload-identity")
            && issue.details.as_ref().unwrap()["objectSpec"] == "value.core.float32"
    }));

    let mut package_deferred = graph.clone();
    package_deferred.nodes[0].implementation = Some(crate::ObjectImplementationRefCurrent {
        provider: crate::ObjectProviderRefCurrent::Package {
            package_id: "user/package".to_owned(),
            lock_entry_id: None,
            version: Some(CURRENT_SCHEMA_VERSION.to_owned()),
        },
        object_id: "user.manipulator".to_owned(),
        interface_digest: None,
    });
    package_deferred.nodes[0].object_spec = Some("user.manipulator 1".to_owned());
    package_deferred.nodes[0].ports.clear();
    validate_project_current(
        &package_deferred,
        &[behavior_definition("user.manipulator")],
    )
    .expect("package-owned object spec remains available to package resolver layers");
}

#[test]
fn surfaces_selector_and_connection_policy_issues_with_specific_codes() {
    let mut selector_graph = render_graph();
    selector_graph.nodes[1].ports[0].port_type = "value.core.message".to_owned();
    selector_graph.nodes[1].ports[0].rate = Some(PortRateCurrent::Control);
    selector_graph.nodes[1].ports[0].message_keys = None;
    let mut selector_output_definition = output_definition();
    selector_output_definition.ports[0] = selector_graph.nodes[1].ports[0].clone();
    let selector_result = validate_project_current(
        &selector_graph,
        &[clear_definition(), selector_output_definition],
    )
    .expect_err("selector-aware input port should fail without selector policy");
    assert!(
        selector_result.iter().any(|issue| {
            issue.code.as_deref() == Some("graph.message-key-policy")
                && issue
                    .message
                    .contains("message-key-aware input port requires messageKeys")
        }),
        "{selector_result:#?}"
    );

    let float_graph = float_pair_graph();
    validate_project_current(&float_graph, &[float_definition()])
        .expect("float value output should connect to message selector inlet");
    assert_eq!(
        float_graph.nodes[1].ports[0].port_type, "value.core.message",
        "node port snapshot remains the resolved definition interface"
    );

    let mut fan_in_graph = render_graph();
    let mut clear_two = fan_in_graph.nodes[0].clone();
    clear_two.id = "clear_two".to_owned();
    fan_in_graph.nodes.push(clear_two);
    fan_in_graph.edges.push(EdgeSpecCurrent {
        id: "edge_clear_two_output".to_owned(),
        source: EdgeEndpointCurrent {
            node_id: "clear_two".to_owned(),
            port_id: "out".to_owned(),
        },
        target: EdgeEndpointCurrent {
            node_id: "output".to_owned(),
            port_id: "in".to_owned(),
        },
        resolved_type: Some("value.core.tensor".to_owned()),
        order: None,
        enabled: None,
        adapter: None,
        feedback: None,
        style_override: None,
        label: None,
        description: None,
    });
    let fan_in_result =
        validate_project_current(&fan_in_graph, &[clear_definition(), output_definition()])
            .expect_err("default input fan-in should fail");
    assert!(
        fan_in_result
            .iter()
            .any(|issue| issue.code.as_deref() == Some("graph.fan-in-cardinality")),
        "{fan_in_result:#?}"
    );

    let mut fan_out_graph = render_graph();
    fan_out_graph.nodes[0].ports[0].fan_out_policy = Some(FanOutPolicyCurrent::Forbid);
    let mut output_two = fan_out_graph.nodes[1].clone();
    output_two.id = "output_two".to_owned();
    fan_out_graph.nodes.push(output_two);
    fan_out_graph.edges.push(EdgeSpecCurrent {
        id: "edge_clear_output_two".to_owned(),
        source: EdgeEndpointCurrent {
            node_id: "clear".to_owned(),
            port_id: "out".to_owned(),
        },
        target: EdgeEndpointCurrent {
            node_id: "output_two".to_owned(),
            port_id: "in".to_owned(),
        },
        resolved_type: Some("value.core.tensor".to_owned()),
        order: None,
        enabled: None,
        adapter: None,
        feedback: None,
        style_override: None,
        label: None,
        description: None,
    });
    let mut fan_out_clear_definition = clear_definition();
    fan_out_clear_definition.ports[0].fan_out_policy = Some(FanOutPolicyCurrent::Forbid);
    let fan_out_result = validate_project_current(
        &fan_out_graph,
        &[fan_out_clear_definition, output_definition()],
    )
    .expect_err("forbidden output fan-out should fail");
    assert!(
        fan_out_result
            .iter()
            .any(|issue| issue.code.as_deref() == Some("graph.fan-out-forbidden")),
        "{fan_out_result:#?}"
    );
}

#[test]
fn rejects_invalid_graph_definitions_and_snapshots() {
    let graph = render_graph();
    let missing = validate_project_current(&graph, &[]).expect_err("missing definitions fail");
    assert_eq!(missing[0].code.as_deref(), Some("node-definition.missing"));
    assert_eq!(
        missing[0].details.as_ref().unwrap()["surface"],
        "node-definition"
    );

    let mut unsupported_graph = render_graph();
    unsupported_graph.schema_version = "9.9.9".to_owned();
    let unsupported_graph_result = validate_project_current(
        &unsupported_graph,
        &[clear_definition(), output_definition()],
    )
    .expect_err("unsupported graph schema should fail");
    assert_eq!(
        unsupported_graph_result[0].code.as_deref(),
        Some("graph.invalid-contract")
    );
    assert_eq!(
        unsupported_graph_result[0].details.as_ref().unwrap()["surface"],
        "graph"
    );
    assert_eq!(
        unsupported_graph_result[0].details.as_ref().unwrap()["expectedSchemaVersion"],
        "0.1.0"
    );
    assert_eq!(
        unsupported_graph_result[0].details.as_ref().unwrap()["receivedSchemaVersion"],
        "9.9.9"
    );

    let mut invalid_definition = clear_definition();
    invalid_definition.permissions.push("network".to_owned());
    let invalid_definition_result =
        validate_project_current(&graph, &[invalid_definition, output_definition()])
            .expect_err("invalid definition should fail");
    let invalid_definition_issue = invalid_definition_result
        .iter()
        .find(|issue| issue.message.contains("unsupported permission: network"))
        .expect("unsupported permission should be reported");
    assert_eq!(
        invalid_definition_issue.code.as_deref(),
        Some("node-definition.invalid-contract")
    );
    assert_eq!(
        invalid_definition_issue.details.as_ref().unwrap()["surface"],
        "node-definition"
    );
    assert_eq!(
        invalid_definition_issue.details.as_ref().unwrap()["expectedSchemaVersion"],
        "0.1.0"
    );
    assert_eq!(
        invalid_definition_issue.details.as_ref().unwrap()["receivedSchemaVersion"],
        "0.1.0"
    );

    let mut mismatch = render_graph();
    mismatch.nodes[0].ports.clear();
    mismatch.nodes[1].ports[0].direction = PortDirectionCurrent::Output;
    mismatch.nodes[1].ports[0].port_type = "value.core.float32".to_owned();
    mismatch.nodes[1].ports.push(PortSpecCurrent {
        id: "extra".to_owned(),
        direction: PortDirectionCurrent::Input,
        port_type: "value.core.tensor".to_owned(),
        label: None,
        rate: None,
        accepts: None,
        min_connections: None,
        max_connections: None,
        merge_policy: None,
        fan_out_policy: None,
        trigger_mode: None,
        message_keys: None,
        default_value: None,
        latch: None,
        required: None,
        style_key: None,
        group: None,
        description: None,
    });
    let mismatch_result =
        validate_project_current(&mismatch, &[clear_definition(), output_definition()])
            .expect_err("snapshot mismatch should fail");
    let messages = mismatch_result
        .iter()
        .map(|issue| issue.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        mismatch_result.iter().all(|issue| issue.code.is_some()),
        "current 0.1 project issues should be structured"
    );
    assert!(messages.contains("missing manifest port"));
    assert!(messages.contains("direction differs from definition"));
    assert!(messages.contains("type value.core.float32"));
    assert!(messages.contains("missing source port"));
    assert!(messages.contains("missing manifest port: output.extra"));

    let mut incompatible = render_graph();
    incompatible.nodes[1].ports[0].port_type = "value.core.message".to_owned();
    incompatible.nodes[1].ports[0].rate = Some(PortRateCurrent::Event);
    let incompatible_result =
        validate_project_current(&incompatible, &[clear_definition(), output_definition()])
            .expect_err("incompatible edge type should fail");
    assert!(incompatible_result.iter().any(|issue| {
        issue.code.as_deref() == Some("graph.edge-incompatible-type")
            && issue.message.contains(
                "incompatible edge clear:out value.core.tensor -> output:in value.core.message",
            )
    }));
}

#[test]
fn labels_all_current_policy_and_execution_variants() {
    for (policy, expected) in [
        (Some(MergePolicyCurrent::Forbid), "forbid"),
        (Some(MergePolicyCurrent::OrderedEvents), "ordered-events"),
        (Some(MergePolicyCurrent::Mix), "mix"),
        (Some(MergePolicyCurrent::Array), "array"),
        (Some(MergePolicyCurrent::Latest), "latest"),
        (Some(MergePolicyCurrent::First), "first"),
        (Some(MergePolicyCurrent::Custom), "custom"),
        (None, "forbid"),
    ] {
        assert_eq!(merge_policy_label(policy.as_ref()), expected);
    }

    for (policy, expected) in [
        (Some(FanOutPolicyCurrent::Allow), "allow"),
        (Some(FanOutPolicyCurrent::Forbid), "forbid"),
        (Some(FanOutPolicyCurrent::Copy), "copy"),
        (Some(FanOutPolicyCurrent::Share), "share"),
        (None, "allow"),
    ] {
        assert_eq!(fan_out_policy_label(policy.as_ref()), expected);
    }

    for (classification, expected) in [
        (CycleValidationCurrent::NoCycle, "no-cycle"),
        (CycleValidationCurrent::ValidFeedback, "valid-feedback"),
        (CycleValidationCurrent::RiskyFeedback, "risky-feedback"),
        (
            CycleValidationCurrent::AmbiguousAlgebraicLoop,
            "ambiguous-algebraic-loop",
        ),
        (CycleValidationCurrent::InvalidCycle, "invalid-cycle"),
    ] {
        assert_eq!(cycle_validation_label(&classification), expected);
    }

    for (model, expected) in [
        (ExecutionModelCurrent::Event, ExecutionModel::Event),
        (ExecutionModelCurrent::Control, ExecutionModel::Control),
        (ExecutionModelCurrent::Frame, ExecutionModel::Frame),
        (
            ExecutionModelCurrent::AudioBlock,
            ExecutionModel::AudioBlock,
        ),
        (
            ExecutionModelCurrent::VideoFrame,
            ExecutionModel::VideoFrame,
        ),
        (ExecutionModelCurrent::GpuPass, ExecutionModel::GpuPass),
        (
            ExecutionModelCurrent::AsyncResource,
            ExecutionModel::AsyncResource,
        ),
        (
            ExecutionModelCurrent::ScriptControl,
            ExecutionModel::ScriptControl,
        ),
        (
            ExecutionModelCurrent::NativePlugin,
            ExecutionModel::NativePlugin,
        ),
    ] {
        assert_eq!(map_execution_model_current(&model), expected);
    }
}
