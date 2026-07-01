use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, json};

use super::domains::find_audio_route_to_output;
use super::offline::run_offline_audio_dsp;
use super::ports::is_audio_signal_edge;
use super::values::control_input_f32_from_params;
use super::*;
use crate::{
    AudioClockBridgeMethod, AudioClockDomainAuthority, AudioEndpointDirection, Edge, GraphDocument,
    NodeDefinition, NodeRegistry, PlanError, ProjectRequestCurrent, RuntimeIssue,
};

fn registry() -> NodeRegistry {
    let mut registry = NodeRegistry::new();
    for definition in [
        audio_source_definition("object.core.audio.osc", "frequency"),
        audio_sig_definition(),
        audio_binary_definition("object.core.audio.operator.mul"),
        audio_unary_definition("object.core.audio.operator.sqrt", "in"),
        audio_snapshot_definition(),
        audio_input_definition(),
        audio_output_definition(),
        audio_clock_boundary_definition("object.core.audio.clock-bridge"),
        audio_clock_boundary_definition("object.core.audio.resample"),
        float_definition(),
        bad_signal_definition(),
    ] {
        registry.insert(definition).unwrap();
    }
    registry
}

fn graph(value: serde_json::Value) -> GraphDocument {
    serde_json::from_value(value).unwrap()
}

#[test]
fn builds_stable_audio_block_plan_with_buffers_and_control_inputs() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "audio-dsp",
      "revision": "7",
      "nodes": [
        float_node("freq", 220.0),
        audio_osc_node("osc"),
        audio_binary_node("mul", "object.core.audio.operator.mul"),
        audio_snapshot_node("snap")
      ],
      "edges": [
        { "from": { "node": "freq", "port": "value" }, "to": { "node": "osc", "port": "frequency" } },
        { "from": { "node": "osc", "port": "out" }, "to": { "node": "mul", "port": "left" } },
        { "from": { "node": "mul", "port": "out" }, "to": { "node": "snap", "port": "signal" } }
      ]
    }));

    let plan = build_audio_dsp_plan(
        &graph,
        &registry(),
        AudioDspPlanOptions {
            block_size: 128,
            sample_rate: 44_100,
        },
    )
    .unwrap();

    assert_eq!(plan.graph_id, "audio-dsp");
    assert_eq!(plan.graph_revision, "7");
    assert_eq!(plan.block_size, 128);
    assert_eq!(plan.sample_rate, 44_100);
    assert_eq!(
        plan.nodes
            .iter()
            .map(|node| (node.order, node.node_id.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "osc"), (1, "mul"), (2, "snap")]
    );
    assert_eq!(
        plan.buffers
            .iter()
            .map(|buffer| {
                (
                    buffer.id.as_str(),
                    buffer.producer_node_id.as_str(),
                    buffer.producer_port_id.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("audio_buffer_0", "osc", "out"),
            ("audio_buffer_1", "mul", "out")
        ]
    );
    assert_eq!(
        plan.edges
            .iter()
            .map(|edge| {
                (
                    edge.from_node.as_str(),
                    edge.from_port.as_str(),
                    edge.to_node.as_str(),
                    edge.to_port.as_str(),
                    edge.buffer_id.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("osc", "out", "mul", "left", "audio_buffer_0"),
            ("mul", "out", "snap", "signal", "audio_buffer_1")
        ]
    );
    assert_eq!(
        plan.nodes[0].control_inputs,
        vec![AudioDspControlInput {
            port_id: "frequency".to_owned(),
            data_kind: "value.core.float32".to_owned(),
            source_node_id: Some("freq".to_owned()),
            source_port_id: Some("value".to_owned()),
        }]
    );
    assert_eq!(
        plan.nodes[1].signal_inputs,
        vec![AudioDspSignalInput {
            port_id: "left".to_owned(),
            source_node_id: "osc".to_owned(),
            source_port_id: "out".to_owned(),
            buffer_id: "audio_buffer_0".to_owned(),
        }]
    );
    assert_eq!(plan.nodes[2].signal_outputs, Vec::new());
    assert_eq!(plan.nodes[2].control_inputs[0].port_id, "trigger");
}

#[test]
fn builds_empty_plan_when_graph_has_no_audio_nodes() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "control-only",
      "revision": "1",
      "nodes": [float_node("value", 1.0)],
      "edges": []
    }));

    let plan = build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

    assert_eq!(plan.nodes, Vec::new());
    assert_eq!(plan.edges, Vec::new());
    assert_eq!(plan.buffers, Vec::new());
    assert_eq!(plan.block_size, DEFAULT_BLOCK_SIZE);
    assert_eq!(plan.sample_rate, DEFAULT_SAMPLE_RATE);
}

#[test]
fn plans_same_clock_domain_audio_input_to_output_as_direct_route() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "same-domain-audio-route",
      "revision": "1",
      "nodes": [
        audio_input_node_with_domain("input", "device:aggregate-a"),
        audio_output_node_with_domain("output", "device:aggregate-a")
      ],
      "edges": [
        { "from": { "node": "input", "port": "out" }, "to": { "node": "output", "port": "left" } }
      ]
    }));

    let plan = build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

    assert_eq!(
        plan.endpoints
            .iter()
            .map(|endpoint| {
                (
                    endpoint.node_id.as_str(),
                    endpoint.direction.clone(),
                    endpoint.clock_domain_id.as_deref(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                "input",
                AudioEndpointDirection::Input,
                Some("device:aggregate-a")
            ),
            (
                "output",
                AudioEndpointDirection::Output,
                Some("device:aggregate-a")
            )
        ]
    );
    assert_eq!(plan.clock_domains.len(), 1);
    assert_eq!(plan.clock_domains[0].id, "device:aggregate-a");
    assert_eq!(
        plan.clock_domains[0].authority,
        AudioClockDomainAuthority::UserConfigured
    );
    assert_eq!(
        plan.partitions
            .iter()
            .map(|partition| {
                (
                    partition.clock_domain_id.as_str(),
                    partition.endpoint_ids.as_slice(),
                )
            })
            .collect::<Vec<_>>(),
        vec![(
            "device:aggregate-a",
            ["input".to_owned(), "output".to_owned()].as_slice()
        )]
    );
    assert_eq!(plan.bridge_plans.len(), 1);
    assert_eq!(plan.bridge_plans[0].method, AudioClockBridgeMethod::Direct);
    assert!(!plan.bridge_plans[0].required);
}

#[test]
fn rejects_independent_audio_input_to_output_without_explicit_bridge() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "missing-clock-bridge",
      "revision": "1",
      "nodes": [
        audio_input_node_with_domain("input", "device:input-clock"),
        audio_output_node_with_domain("output", "device:output-clock")
      ],
      "edges": [
        { "from": { "node": "input", "port": "out" }, "to": { "node": "output", "port": "left" } }
      ]
    }));

    let error =
        build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap_err();

    assert!(matches!(
        error,
        AudioDspPlanError::ClockDomainCrossingRequiresBridge {
            source_node_id,
            target_node_id,
            source_clock_domain_id,
            target_clock_domain_id,
            ..
        } if source_node_id == "input"
            && target_node_id == "output"
            && source_clock_domain_id == "device:input-clock"
            && target_clock_domain_id == "device:output-clock"
    ));
}

#[test]
fn unconnected_audio_endpoint_route_has_no_bridge_plan() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "unconnected-endpoints",
      "revision": "1",
      "nodes": [
        audio_input_node("input"),
        audio_output_node("output")
      ],
      "edges": []
    }));

    let plan = build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

    assert_eq!(plan.endpoints.len(), 2);
    assert_eq!(plan.bridge_plans, Vec::new());
}

#[test]
fn partitions_audio_nodes_under_input_domain_when_no_output_exists() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "input-only-partition",
      "revision": "1",
      "nodes": [
        audio_input_node("input"),
        audio_sig_node("sig", 0.25)
      ],
      "edges": []
    }));

    let plan = build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

    assert_eq!(plan.bridge_plans, Vec::new());
    assert_eq!(
        plan.partitions
            .iter()
            .map(|partition| {
                (
                    partition.clock_domain_id.as_str(),
                    partition.endpoint_ids.clone(),
                    partition.node_ids.clone(),
                )
            })
            .collect::<Vec<_>>(),
        vec![(
            "endpoint:input",
            vec!["input".to_owned()],
            vec!["input".to_owned(), "sig".to_owned()]
        )]
    );
}

#[test]
fn plans_explicit_clock_bridge_for_independent_audio_domains() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "clock-bridge-route",
      "revision": "1",
      "nodes": [
        audio_input_node_with_domain("input", "device:input-clock"),
        audio_clock_boundary_node("bridge", "object.core.audio.clock-bridge"),
        audio_output_node_with_domain("output", "device:output-clock")
      ],
      "edges": [
        { "from": { "node": "input", "port": "out" }, "to": { "node": "bridge", "port": "in" } },
        { "from": { "node": "bridge", "port": "out" }, "to": { "node": "output", "port": "left" } }
      ]
    }));

    let plan = build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

    assert_eq!(plan.bridge_plans.len(), 1);
    assert!(plan.bridge_plans[0].required);
    assert_eq!(
        plan.bridge_plans[0].source_clock_domain_id,
        "device:input-clock"
    );
    assert_eq!(
        plan.bridge_plans[0].target_clock_domain_id,
        "device:output-clock"
    );
    assert_eq!(
        plan.bridge_plans[0].method,
        AudioClockBridgeMethod::ClockBridge
    );
    assert_eq!(
        plan.bridge_plans[0].bridge_node_id.as_deref(),
        Some("bridge")
    );
}

#[test]
fn plans_default_endpoint_domains_as_unavailable_with_explicit_bridge() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "default-endpoint-domains",
      "revision": "1",
      "nodes": [
        audio_input_node("input"),
        audio_clock_boundary_node("bridge", "object.core.audio.clock-bridge"),
        audio_output_node("output")
      ],
      "edges": [
        { "from": { "node": "input", "port": "out" }, "to": { "node": "bridge", "port": "in" } },
        { "from": { "node": "bridge", "port": "out" }, "to": { "node": "output", "port": "left" } }
      ]
    }));

    let plan = build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

    assert_eq!(
        plan.clock_domains
            .iter()
            .map(|domain| (domain.id.as_str(), domain.authority.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("endpoint:input", AudioClockDomainAuthority::Unavailable),
            ("endpoint:output", AudioClockDomainAuthority::Unavailable)
        ]
    );
    assert_eq!(
        plan.bridge_plans[0].source_clock_domain_id,
        "endpoint:input"
    );
    assert_eq!(
        plan.bridge_plans[0].target_clock_domain_id,
        "endpoint:output"
    );
}

#[test]
fn plans_explicit_resample_for_independent_audio_domains() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "resample-route",
      "revision": "1",
      "nodes": [
        audio_input_node_with_domain("input", "device:input-clock"),
        audio_clock_boundary_node("resample", "object.core.audio.resample"),
        audio_output_node_with_domain("output", "device:output-clock")
      ],
      "edges": [
        { "from": { "node": "input", "port": "out" }, "to": { "node": "resample", "port": "in" } },
        { "from": { "node": "resample", "port": "out" }, "to": { "node": "output", "port": "left" } }
      ]
    }));

    let plan = build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

    assert_eq!(plan.bridge_plans.len(), 1);
    assert_eq!(
        plan.bridge_plans[0].method,
        AudioClockBridgeMethod::Resample
    );
    assert_eq!(
        plan.bridge_plans[0].bridge_node_id.as_deref(),
        Some("resample")
    );
}

#[test]
fn audio_route_helper_skips_cycles_and_non_signal_edges() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "route-helper",
      "revision": "1",
      "nodes": [
        audio_input_node("input"),
        audio_clock_boundary_node("bridge_a", "object.core.audio.clock-bridge"),
        audio_clock_boundary_node("bridge_b", "object.core.audio.clock-bridge"),
        audio_output_node("output"),
        float_node("not_signal", 1.0)
      ],
      "edges": [
        { "from": { "node": "input", "port": "out" }, "to": { "node": "not_signal", "port": "value" } },
        { "from": { "node": "input", "port": "out" }, "to": { "node": "bridge_a", "port": "in" } },
        { "from": { "node": "bridge_a", "port": "out" }, "to": { "node": "bridge_b", "port": "in" } },
        { "from": { "node": "bridge_b", "port": "out" }, "to": { "node": "output", "port": "left" } },
        { "from": { "node": "bridge_b", "port": "out" }, "to": { "node": "bridge_a", "port": "in" } }
      ]
    }));
    let nodes_by_id = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();

    let route = find_audio_route_to_output(&graph, &nodes_by_id, "input", "output").unwrap();
    let missing = find_audio_route_to_output(&graph, &nodes_by_id, "output", "input");

    assert_eq!(route.explicit_bridge_node_id.as_deref(), Some("bridge_b"));
    assert_eq!(
        route.explicit_bridge_kind.as_deref(),
        Some("object.core.audio.clock-bridge")
    );
    assert_eq!(missing, None);
}

#[test]
fn rejects_invalid_options_and_projects() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "invalid-options",
      "revision": "1",
      "nodes": [float_node("value", 1.0)],
      "edges": []
    }));
    let registry = registry();

    let block_error = build_audio_dsp_plan(
        &graph,
        &registry,
        AudioDspPlanOptions {
            block_size: 0,
            sample_rate: 48_000,
        },
    )
    .unwrap_err();
    let rate_error = build_audio_dsp_plan(
        &graph,
        &registry,
        AudioDspPlanOptions {
            block_size: 64,
            sample_rate: 0,
        },
    )
    .unwrap_err();
    let project_error =
        build_audio_dsp_plan(&graph, &NodeRegistry::new(), AudioDspPlanOptions::default())
            .unwrap_err();

    assert_eq!(
        block_error.issues()[0].code.as_deref(),
        Some("audio-dsp.invalid-block-size")
    );
    assert_eq!(
        rate_error.issues()[0].code.as_deref(),
        Some("audio-dsp.invalid-sample-rate")
    );
    assert!(!project_error.issues().is_empty());
    assert!(matches!(
        block_error,
        AudioDspPlanError::InvalidBlockSize { .. }
    ));
    assert!(matches!(
        rate_error,
        AudioDspPlanError::InvalidSampleRate { .. }
    ));
    assert!(matches!(
        project_error,
        AudioDspPlanError::InvalidProject { .. }
    ));
}

#[test]
fn dsp_error_helpers_preserve_structured_issues() {
    let issue = RuntimeIssue::structured_error(
        "audio-dsp.test",
        "test issue",
        json!({ "source": "dsp-error-test" }),
    );
    let invalid_project = AudioDspPlanError::from_issues(vec![issue.clone()]);

    assert_eq!(invalid_project.issues(), vec![issue.clone()]);

    let plan_error = AudioDspPlanError::from_plan_error(PlanError::Cycle {
        nodes: "osc -> osc".to_owned(),
    });
    assert_eq!(
        plan_error.issues()[0].code.as_deref(),
        Some("audio-dsp.plan")
    );
    assert_eq!(
        AudioOfflineDspError::Plan(plan_error).issues()[0]
            .code
            .as_deref(),
        Some("audio-dsp.plan")
    );

    assert_eq!(
        AudioRealtimeDspError::Plan(invalid_project).issues(),
        vec![issue]
    );
}

#[test]
fn rejects_signal_ports_outside_audio_block_nodes() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "bad-signal",
      "revision": "1",
      "nodes": [bad_signal_node("bad")],
      "edges": []
    }));

    let error =
        build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap_err();

    assert_eq!(
        error.issues()[0].code.as_deref(),
        Some("audio-dsp.signal-port-outside-audio-block")
    );
    assert!(matches!(
        error,
        AudioDspPlanError::SignalPortOutsideAudioBlock { .. }
    ));
    assert!(error.to_string().contains("bad.out"));
}

#[test]
fn signal_edge_helper_ignores_missing_endpoint_ports() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "missing-edge-ports",
      "revision": "1",
      "nodes": [audio_osc_node("osc"), audio_binary_node("mul", "object.core.audio.operator.mul")],
      "edges": []
    }));
    let missing_from = serde_json::from_value::<Edge>(json!({
      "from": { "node": "osc", "port": "missing" },
      "to": { "node": "mul", "port": "left" }
    }))
    .unwrap();
    let missing_to = serde_json::from_value::<Edge>(json!({
      "from": { "node": "osc", "port": "out" },
      "to": { "node": "mul", "port": "missing" }
    }))
    .unwrap();

    assert!(!is_audio_signal_edge(&missing_from, &graph));
    assert!(!is_audio_signal_edge(&missing_to, &graph));
}

#[test]
fn runs_offline_sig_mul_snapshot_blocks() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "offline-mul",
      "revision": "3",
      "nodes": [
        audio_sig_node("left", 2.0),
        audio_sig_node("right", 0.5),
        audio_binary_node("mul", "object.core.audio.operator.mul"),
        audio_snapshot_node("snap")
      ],
      "edges": [
        { "from": { "node": "left", "port": "out" }, "to": { "node": "mul", "port": "left" } },
        { "from": { "node": "right", "port": "out" }, "to": { "node": "mul", "port": "right" } },
        { "from": { "node": "mul", "port": "out" }, "to": { "node": "snap", "port": "signal" } }
      ]
    }));

    let report = run_offline_audio_dsp(
        &graph,
        &registry(),
        AudioOfflineDspOptions {
            blocks: 2,
            plan: AudioDspPlanOptions {
                block_size: 4,
                sample_rate: 48_000,
            },
        },
    )
    .unwrap();

    assert_eq!(report.graph_id, "offline-mul");
    assert_eq!(report.graph_revision, "3");
    assert_eq!(report.block_size, 4);
    assert_eq!(report.sample_rate, 48_000);
    assert_eq!(report.blocks.len(), 2);
    assert_eq!(
        rendered_samples(&report, 0, "mul", "out"),
        vec![1.0, 1.0, 1.0, 1.0]
    );
    assert_eq!(
        report
            .snapshots
            .iter()
            .map(|snapshot| (
                snapshot.block_index,
                snapshot.node_id.as_str(),
                snapshot.value
            ))
            .collect::<Vec<_>>(),
        vec![(0, "snap", 1.0), (1, "snap", 1.0)]
    );
}

#[test]
fn runs_offline_oscillator_with_control_frequency_and_phase_continuity() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "offline-osc",
      "revision": "5",
      "nodes": [
        float_node("freq", 1.0),
        audio_osc_node("osc")
      ],
      "edges": [
        { "from": { "node": "freq", "port": "value" }, "to": { "node": "osc", "port": "frequency" } }
      ]
    }));

    let report = run_offline_audio_dsp(
        &graph,
        &registry(),
        AudioOfflineDspOptions {
            blocks: 2,
            plan: AudioDspPlanOptions {
                block_size: 4,
                sample_rate: 4,
            },
        },
    )
    .unwrap();

    assert_samples_close(
        &rendered_samples(&report, 0, "osc", "out"),
        &[0.0, 1.0, 0.0, -1.0],
    );
    assert_samples_close(
        &rendered_samples(&report, 1, "osc", "out"),
        &[0.0, 1.0, 0.0, -1.0],
    );
}

#[test]
fn offline_execution_uses_param_frequency_and_zero_for_missing_signal_input() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "offline-fallbacks",
      "revision": "2",
      "nodes": [
        audio_osc_node("osc"),
        audio_sig_node("left", 3.0),
        audio_binary_node("mul", "object.core.audio.operator.mul")
      ],
      "edges": [
        { "from": { "node": "left", "port": "out" }, "to": { "node": "mul", "port": "left" } }
      ]
    }));

    let report = run_offline_audio_dsp(
        &graph,
        &registry(),
        AudioOfflineDspOptions {
            blocks: 1,
            plan: AudioDspPlanOptions {
                block_size: 4,
                sample_rate: 1_760,
            },
        },
    )
    .unwrap();

    assert_samples_close(
        &rendered_samples(&report, 0, "osc", "out"),
        &[0.0, 1.0, 0.0, -1.0],
    );
    assert_eq!(
        rendered_samples(&report, 0, "mul", "out"),
        vec![0.0, 0.0, 0.0, 0.0]
    );
}

#[test]
fn rejects_invalid_offline_options_and_unsupported_nodes() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "offline-errors",
      "revision": "1",
      "nodes": [
        audio_sig_node("input", -4.0),
        audio_unary_node("sqrt", "object.core.audio.operator.sqrt")
      ],
      "edges": [
        { "from": { "node": "input", "port": "out" }, "to": { "node": "sqrt", "port": "in" } }
      ]
    }));

    let block_error = run_offline_audio_dsp(
        &graph,
        &registry(),
        AudioOfflineDspOptions {
            blocks: 0,
            plan: AudioDspPlanOptions::default(),
        },
    )
    .unwrap_err();
    let unsupported_error =
        run_offline_audio_dsp(&graph, &registry(), AudioOfflineDspOptions::default()).unwrap_err();

    assert!(matches!(
        &block_error,
        AudioOfflineDspError::InvalidBlockCount { .. }
    ));
    assert!(matches!(
        &unsupported_error,
        AudioOfflineDspError::UnsupportedNodeKind { .. }
    ));
    assert_eq!(
        block_error.issues()[0].code.as_deref(),
        Some("audio-dsp.invalid-offline-block-count")
    );
    assert_eq!(
        unsupported_error.issues()[0].code.as_deref(),
        Some("audio-dsp.offline-unsupported-node-kind")
    );
    assert!(
        unsupported_error
            .to_string()
            .contains("object.core.audio.operator.sqrt")
    );
}

#[test]
fn realtime_executor_writes_stereo_output_from_audio_output_sink() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "realtime-output",
      "revision": "1",
      "nodes": [
        audio_sig_node("left", 0.25),
        audio_sig_node("right", -0.5),
        audio_output_node("out")
      ],
      "edges": [
        { "from": { "node": "left", "port": "out" }, "to": { "node": "out", "port": "left" } },
        { "from": { "node": "right", "port": "out" }, "to": { "node": "out", "port": "right" } }
      ]
    }));
    let mut executor = AudioRealtimeDspExecutor::new_from_graph(
        &graph,
        &registry(),
        AudioRealtimeDspOptions {
            plan: AudioDspPlanOptions {
                block_size: 4,
                sample_rate: 48_000,
            },
            channels: 2,
        },
    )
    .unwrap();
    let mut output = vec![99.0; 8];

    executor.process_interleaved_output(&mut output);

    assert_eq!(executor.channels(), 2);
    assert_eq!(executor.plan().block_size, 4);
    assert_eq!(output, vec![0.25, -0.5, 0.25, -0.5, 0.25, -0.5, 0.25, -0.5]);

    let mut empty = Vec::new();
    executor.process_interleaved_output(&mut empty);
    assert!(empty.is_empty());
}

#[test]
fn realtime_executor_runs_mul_before_audio_output_sink() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "realtime-mul",
      "revision": "1",
      "nodes": [
        audio_sig_node("left", 2.0),
        audio_sig_node("right", 0.5),
        audio_binary_node("mul", "object.core.audio.operator.mul"),
        audio_output_node("out")
      ],
      "edges": [
        { "from": { "node": "left", "port": "out" }, "to": { "node": "mul", "port": "left" } },
        { "from": { "node": "right", "port": "out" }, "to": { "node": "mul", "port": "right" } },
        { "from": { "node": "mul", "port": "out" }, "to": { "node": "out", "port": "left" } },
        { "from": { "node": "mul", "port": "out" }, "to": { "node": "out", "port": "right" } }
      ]
    }));
    let mut executor = AudioRealtimeDspExecutor::new_from_graph(
        &graph,
        &registry(),
        AudioRealtimeDspOptions {
            plan: AudioDspPlanOptions {
                block_size: 4,
                sample_rate: 48_000,
            },
            channels: 2,
        },
    )
    .unwrap();
    let mut output = vec![0.0; 8];

    executor.process_interleaved_output(&mut output);

    assert_eq!(output, vec![1.0; 8]);
}

#[test]
fn realtime_executor_keeps_oscillator_phase_across_partial_callbacks() {
    let graph = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "realtime-osc",
      "revision": "1",
      "nodes": [
        audio_osc_node("osc"),
        audio_output_node("out")
      ],
      "edges": [
        { "from": { "node": "osc", "port": "out" }, "to": { "node": "out", "port": "left" } }
      ]
    }));
    let mut executor = AudioRealtimeDspExecutor::new_from_graph(
        &graph,
        &registry(),
        AudioRealtimeDspOptions {
            plan: AudioDspPlanOptions {
                block_size: 4,
                sample_rate: 1_760,
            },
            channels: 2,
        },
    )
    .unwrap();
    let mut output = vec![0.0; 12];

    executor.process_interleaved_output(&mut output);
    let left = output
        .chunks_exact(2)
        .map(|frame| frame[0])
        .collect::<Vec<_>>();
    let right = output
        .chunks_exact(2)
        .map(|frame| frame[1])
        .collect::<Vec<_>>();

    assert_samples_close(&left, &[0.0, 1.0, 0.0, -1.0, 0.0, 1.0]);
    assert_eq!(right, vec![0.0; 6]);
}

#[test]
fn realtime_executor_reports_output_and_kind_errors_without_processing() {
    let graph_without_output = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "no-output",
      "revision": "1",
      "nodes": [audio_sig_node("sig", 1.0)],
      "edges": []
    }));
    let duplicate_outputs = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "duplicate-output",
      "revision": "1",
      "nodes": [audio_output_node("a"), audio_output_node("b")],
      "edges": []
    }));
    let unsupported = graph(json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "unsupported-realtime",
      "revision": "1",
      "nodes": [
        audio_unary_node("sqrt", "object.core.audio.operator.sqrt"),
        audio_output_node("out")
      ],
      "edges": [
        { "from": { "node": "sqrt", "port": "out" }, "to": { "node": "out", "port": "left" } }
      ]
    }));
    let invalid_channels = AudioRealtimeDspExecutor::new_from_graph(
        &graph_without_output,
        &registry(),
        AudioRealtimeDspOptions {
            plan: AudioDspPlanOptions::default(),
            channels: 0,
        },
    )
    .unwrap_err();
    let missing_output = AudioRealtimeDspExecutor::new_from_graph(
        &graph_without_output,
        &registry(),
        AudioRealtimeDspOptions::default(),
    )
    .unwrap_err();
    let duplicate_output = AudioRealtimeDspExecutor::new_from_graph(
        &duplicate_outputs,
        &registry(),
        AudioRealtimeDspOptions::default(),
    )
    .unwrap_err();
    let unsupported_kind = AudioRealtimeDspExecutor::new_from_graph(
        &unsupported,
        &registry(),
        AudioRealtimeDspOptions::default(),
    )
    .unwrap_err();

    assert_eq!(
        invalid_channels.issues()[0].code.as_deref(),
        Some("audio-dsp.invalid-realtime-channel-count")
    );
    assert_eq!(
        missing_output.issues()[0].code.as_deref(),
        Some("audio-dsp.realtime-output-count")
    );
    assert_eq!(
        unsupported_kind.issues()[0].code.as_deref(),
        Some("audio-dsp.realtime-unsupported-node-kind")
    );
    assert!(matches!(
        invalid_channels,
        AudioRealtimeDspError::InvalidChannelCount { .. }
    ));
    assert!(matches!(
        missing_output,
        AudioRealtimeDspError::OutputCount { count: 0, .. }
    ));
    assert!(matches!(
        duplicate_output,
        AudioRealtimeDspError::OutputCount { count: 2, .. }
    ));
    assert!(matches!(
        unsupported_kind,
        AudioRealtimeDspError::UnsupportedNodeKind { .. }
    ));
}

#[test]
fn realtime_control_input_helper_handles_absent_sources() {
    let node = AudioDspPlanNode {
        node_id: "osc".to_owned(),
        kind: "object.core.audio.osc".to_owned(),
        kind_version: "0.1.0".to_owned(),
        order: 0,
        params: Map::new(),
        signal_inputs: Vec::new(),
        control_inputs: vec![
            AudioDspControlInput {
                port_id: "other".to_owned(),
                data_kind: "value.core.float32".to_owned(),
                source_node_id: None,
                source_port_id: None,
            },
            AudioDspControlInput {
                port_id: "frequency".to_owned(),
                data_kind: "value.core.float32".to_owned(),
                source_node_id: None,
                source_port_id: None,
            },
        ],
        signal_outputs: Vec::new(),
    };
    let mut node_params = BTreeMap::new();

    assert_eq!(
        control_input_f32_from_params(&node, "frequency", &node_params),
        None
    );
    assert_eq!(
        control_input_f32_from_params(&node, "missing", &node_params),
        None
    );

    let mut node_with_missing_source = node.clone();
    node_with_missing_source.control_inputs[1].source_node_id = Some("missing".to_owned());
    assert_eq!(
        control_input_f32_from_params(&node_with_missing_source, "frequency", &node_params),
        None
    );

    node_params.insert(
        "missing".to_owned(),
        Map::from_iter([("value".to_owned(), json!(12.5))]),
    );
    assert_eq!(
        control_input_f32_from_params(&node_with_missing_source, "frequency", &node_params),
        Some(12.5)
    );
}

#[test]
fn public_current_audio_dsp_entrypoints_validate_and_lower_internally() {
    let request = project_request_current(
        json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "public-current-audio",
          "revision": "1",
          "nodes": [
            audio_sig_node_current("left", 0.25),
            audio_sig_node_current("right", -0.5),
            audio_output_node_current("out")
          ],
          "edges": [
            {
              "id": "edge_left_out",
              "source": { "nodeId": "left", "portId": "out" },
              "target": { "nodeId": "out", "portId": "left" },
              "resolvedType": "value.core.float32"
            },
            {
              "id": "edge_right_out",
              "source": { "nodeId": "right", "portId": "out" },
              "target": { "nodeId": "out", "portId": "right" },
              "resolvedType": "value.core.float32"
            }
          ]
        }),
        vec![
            audio_sig_definition_current(),
            audio_output_definition_current(),
        ],
    );

    let plan = build_audio_dsp_plan_current(
        &request,
        AudioDspPlanOptions {
            block_size: 4,
            sample_rate: 48_000,
        },
    )
    .unwrap();
    assert_eq!(plan.graph_id, "public-current-audio");
    assert_eq!(
        plan.nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>(),
        vec!["left", "right", "out"]
    );

    let mut executor = AudioRealtimeDspExecutor::new(
        &request,
        AudioRealtimeDspOptions {
            plan: AudioDspPlanOptions {
                block_size: 4,
                sample_rate: 48_000,
            },
            channels: 2,
        },
    )
    .unwrap();
    let mut output = vec![0.0; 8];
    executor.process_interleaved_output(&mut output);
    assert_eq!(output, vec![0.25, -0.5, 0.25, -0.5, 0.25, -0.5, 0.25, -0.5]);

    let offline_request = project_request_current(
        json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "public-current-offline-audio",
          "revision": "1",
          "nodes": [
            audio_sig_node_current("sig", 0.75),
            audio_snapshot_node_current("snap")
          ],
          "edges": [
            {
              "id": "edge_sig_snap",
              "source": { "nodeId": "sig", "portId": "out" },
              "target": { "nodeId": "snap", "portId": "signal" },
              "resolvedType": "value.core.float32"
            }
          ]
        }),
        vec![
            audio_sig_definition_current(),
            audio_snapshot_definition_current(),
        ],
    );
    let report = run_offline_audio_dsp_current(
        &offline_request,
        AudioOfflineDspOptions {
            blocks: 1,
            plan: AudioDspPlanOptions {
                block_size: 4,
                sample_rate: 48_000,
            },
        },
    )
    .unwrap();
    assert_eq!(report.snapshots[0].value, 0.75);

    let mut unsupported_request = request.clone();
    unsupported_request.graph.schema_version = "9.9.9".to_owned();
    let error = build_audio_dsp_plan_current(&unsupported_request, AudioDspPlanOptions::default())
        .unwrap_err();
    assert!(
        error
            .issues()
            .iter()
            .any(|issue| { issue.code.as_deref() == Some("project.unsupported-schema-version") })
    );
}

fn audio_source_definition(id: &str, input_port: &str) -> NodeDefinition {
    node_definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": id,
      "version": "0.1.0",
      "displayName": id,
      "category": "Audio",
      "ports": [
        { "id": input_port, "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn audio_sig_definition() -> NodeDefinition {
    node_definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.audio.sig",
      "version": "0.1.0",
      "displayName": "sig~",
      "category": "Audio",
      "ports": [
        { "id": "value", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn audio_binary_definition(id: &str) -> NodeDefinition {
    node_definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": id,
      "version": "0.1.0",
      "displayName": id,
      "category": "Audio",
      "ports": [
        { "id": "left", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "right", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn audio_unary_definition(id: &str, input_port: &str) -> NodeDefinition {
    node_definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": id,
      "version": "0.1.0",
      "displayName": id,
      "category": "Audio",
      "ports": [
        { "id": input_port, "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn audio_snapshot_definition() -> NodeDefinition {
    node_definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.audio.snapshot",
      "version": "0.1.0",
      "displayName": "snapshot~",
      "category": "Audio",
      "ports": [
        { "id": "signal", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "trigger", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.message" }, "activation": "trigger" },
        { "id": "value", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn audio_input_definition() -> NodeDefinition {
    node_definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.audio.input",
      "version": "0.1.0",
      "displayName": "adc~",
      "category": "Audio",
      "ports": [
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn audio_output_definition() -> NodeDefinition {
    node_definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.audio.output",
      "version": "0.1.0",
      "displayName": "dac~",
      "category": "Audio",
      "ports": [
        { "id": "left", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "right", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn audio_clock_boundary_definition(id: &str) -> NodeDefinition {
    node_definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": id,
      "version": "0.1.0",
      "displayName": id,
      "category": "Audio",
      "ports": [
        { "id": "in", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn float_definition() -> NodeDefinition {
    node_definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.float",
      "version": "0.1.0",
      "displayName": "Float",
      "category": "Core",
      "ports": [
        { "id": "value", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
      ],
      "execution": { "model": "control" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn bad_signal_definition() -> NodeDefinition {
    node_definition(json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "test.bad-signal",
      "version": "0.1.0",
      "displayName": "Bad Signal",
      "category": "Test",
      "ports": [
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ],
      "execution": { "model": "control" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    }))
}

fn node_definition(value: serde_json::Value) -> NodeDefinition {
    serde_json::from_value(value).unwrap()
}

fn project_request_current(
    graph: serde_json::Value,
    nodes: Vec<serde_json::Value>,
) -> ProjectRequestCurrent {
    ProjectRequestCurrent {
        document: None,
        graph: serde_json::from_value(graph).unwrap(),
        nodes: nodes
            .into_iter()
            .map(|node| serde_json::from_value::<crate::NodeDefinitionCurrent>(node).unwrap())
            .collect(),
        patch_library: Vec::new(),
        view_state: None,
    }
}

fn audio_sig_definition_current() -> serde_json::Value {
    json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.audio.sig",
      "version": "0.1.0",
      "displayName": "sig~",
      "category": "Audio",
      "ports": [
        { "id": "value", "direction": "input", "type": "value.core.float32" },
        { "id": "out", "direction": "output", "type": "value.core.float32", "rate": "audio" }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    })
}

fn audio_snapshot_definition_current() -> serde_json::Value {
    json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.audio.snapshot",
      "version": "0.1.0",
      "displayName": "snapshot~",
      "category": "Audio",
      "ports": [
        { "id": "signal", "direction": "input", "type": "value.core.float32", "rate": "audio" },
        {
          "id": "trigger",
          "direction": "input",
          "type": "value.core.message",
          "rate": "event",
          "triggerMode": "trigger",
          "accepts": ["value.core.bang"],
          "messageKeys": {
            "accepted": ["bang"],
            "trigger": ["bang"]
          }
        },
        { "id": "value", "direction": "output", "type": "value.core.float32" }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    })
}

fn audio_output_definition_current() -> serde_json::Value {
    json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.audio.output",
      "version": "0.1.0",
      "displayName": "dac~",
      "category": "Audio",
      "ports": [
        { "id": "left", "direction": "input", "type": "value.core.float32", "rate": "audio" },
        { "id": "right", "direction": "input", "type": "value.core.float32", "rate": "audio" }
      ],
      "execution": { "model": "audio_block" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    })
}

fn current_core_node_json(
    id: &str,
    object_id: &str,
    params: serde_json::Value,
    ports: serde_json::Value,
) -> serde_json::Value {
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

fn audio_sig_node_current(id: &str, value: f64) -> serde_json::Value {
    current_core_node_json(
        id,
        "audio.sig",
        json!({ "value": value }),
        json!([
          { "id": "value", "direction": "input", "type": "value.core.float32" },
          { "id": "out", "direction": "output", "type": "value.core.float32", "rate": "audio" }
        ]),
    )
}

fn audio_snapshot_node_current(id: &str) -> serde_json::Value {
    current_core_node_json(
        id,
        "audio.snapshot",
        json!({}),
        json!([
          { "id": "signal", "direction": "input", "type": "value.core.float32", "rate": "audio" },
          {
            "id": "trigger",
            "direction": "input",
            "type": "value.core.message",
            "rate": "event",
            "triggerMode": "trigger",
            "accepts": ["value.core.bang"],
            "messageKeys": {
              "accepted": ["bang"],
              "trigger": ["bang"]
            }
          },
          { "id": "value", "direction": "output", "type": "value.core.float32" }
        ]),
    )
}

fn audio_output_node_current(id: &str) -> serde_json::Value {
    current_core_node_json(
        id,
        "audio.output",
        json!({}),
        json!([
          { "id": "left", "direction": "input", "type": "value.core.float32", "rate": "audio" },
          { "id": "right", "direction": "input", "type": "value.core.float32", "rate": "audio" }
        ]),
    )
}

fn float_node(id: &str, value: f64) -> serde_json::Value {
    json!({
      "id": id,
      "kind": "object.core.float",
      "kindVersion": "0.1.0",
      "params": { "value": value },
      "ports": [
        { "id": "value", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
      ]
    })
}

fn audio_osc_node(id: &str) -> serde_json::Value {
    json!({
      "id": id,
      "kind": "object.core.audio.osc",
      "kindVersion": "0.1.0",
      "params": { "frequency": 440.0 },
      "ports": [
        { "id": "frequency", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ]
    })
}

fn audio_sig_node(id: &str, value: f64) -> serde_json::Value {
    json!({
      "id": id,
      "kind": "object.core.audio.sig",
      "kindVersion": "0.1.0",
      "params": { "value": value },
      "ports": [
        { "id": "value", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ]
    })
}

fn audio_binary_node(id: &str, kind: &str) -> serde_json::Value {
    json!({
      "id": id,
      "kind": kind,
      "kindVersion": "0.1.0",
      "params": {},
      "ports": [
        { "id": "left", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "right", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ]
    })
}

fn audio_unary_node(id: &str, kind: &str) -> serde_json::Value {
    json!({
      "id": id,
      "kind": kind,
      "kindVersion": "0.1.0",
      "params": {},
      "ports": [
        { "id": "in", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ]
    })
}

fn audio_snapshot_node(id: &str) -> serde_json::Value {
    json!({
      "id": id,
      "kind": "object.core.audio.snapshot",
      "kindVersion": "0.1.0",
      "params": {},
      "ports": [
        { "id": "signal", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "trigger", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.message" }, "activation": "trigger" },
        { "id": "value", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
      ]
    })
}

fn audio_output_node(id: &str) -> serde_json::Value {
    json!({
      "id": id,
      "kind": "object.core.audio.output",
      "kindVersion": "0.1.0",
      "params": {},
      "ports": [
        { "id": "left", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "right", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" }
      ]
    })
}

fn audio_input_node(id: &str) -> serde_json::Value {
    json!({
      "id": id,
      "kind": "object.core.audio.input",
      "kindVersion": "0.1.0",
      "params": {},
      "ports": [
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ]
    })
}

fn audio_input_node_with_domain(id: &str, clock_domain: &str) -> serde_json::Value {
    json!({
      "id": id,
      "kind": "object.core.audio.input",
      "kindVersion": "0.1.0",
      "params": { "clockDomain": clock_domain },
      "ports": [
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ]
    })
}

fn audio_output_node_with_domain(id: &str, clock_domain: &str) -> serde_json::Value {
    json!({
      "id": id,
      "kind": "object.core.audio.output",
      "kindVersion": "0.1.0",
      "params": { "clockDomain": clock_domain },
      "ports": [
        { "id": "left", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "right", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" }
      ]
    })
}

fn audio_clock_boundary_node(id: &str, kind: &str) -> serde_json::Value {
    json!({
      "id": id,
      "kind": kind,
      "kindVersion": "0.1.0",
      "params": {},
      "ports": [
        { "id": "in", "direction": "input", "type": { "flow": "signal", "dataKind": "value.core.float32" }, "activation": "latched" },
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ]
    })
}

fn bad_signal_node(id: &str) -> serde_json::Value {
    json!({
      "id": id,
      "kind": "test.bad-signal",
      "kindVersion": "0.1.0",
      "params": {},
      "ports": [
        { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "value.core.float32" } }
      ]
    })
}

fn rendered_samples(
    report: &AudioOfflineDspReport,
    block_index: usize,
    node_id: &str,
    port_id: &str,
) -> Vec<f32> {
    report.blocks[block_index]
        .buffers
        .iter()
        .find(|buffer| buffer.producer_node_id == node_id && buffer.producer_port_id == port_id)
        .unwrap()
        .samples
        .clone()
}

fn assert_samples_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected.iter()) {
        assert!(
            (actual - expected).abs() < 0.000_001,
            "expected {expected}, got {actual}"
        );
    }
}
