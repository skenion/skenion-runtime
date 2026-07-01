use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::json;

use super::domains::{
    audio_clock_bridge_plans, audio_clock_domains, audio_endpoint_plan_nodes, audio_endpoints,
    audio_graph_partitions,
};
use super::ports::{is_audio_signal_edge, is_audio_signal_output, is_audio_signal_port};
use super::{
    AudioDspBuffer, AudioDspControlInput, AudioDspPlan, AudioDspPlanEdge, AudioDspPlanError,
    AudioDspPlanNode, AudioDspPlanOptions, AudioDspSignalInput, AudioDspSignalOutput,
    DEFAULT_SAMPLE_FORMAT,
};
use crate::{
    ExecutionModel, ExecutionPlan, GraphDocument, GraphNode, NodeRegistry, PortDirection,
    ProjectRequestCurrent, RuntimeIssue, build_execution_plan,
    build_execution_plan_request_current, expand_project_graph_current, schema_version_issue,
    validate_project,
};

pub fn build_audio_dsp_plan_current(
    request: &ProjectRequestCurrent,
    options: AudioDspPlanOptions,
) -> Result<AudioDspPlan, AudioDspPlanError> {
    build_audio_dsp_plan_with_graph_current(request, options).map(|(plan, _)| plan)
}

pub(crate) fn build_audio_dsp_plan_with_graph_current(
    request: &ProjectRequestCurrent,
    options: AudioDspPlanOptions,
) -> Result<(AudioDspPlan, GraphDocument), AudioDspPlanError> {
    validate_audio_dsp_plan_options(options)?;
    validate_audio_dsp_request_schema_versions_current(request)?;
    let (execution_plan, _issues) =
        build_execution_plan_request_current(request).map_err(AudioDspPlanError::from_issues)?;
    let expanded_graph = expand_project_graph_current(&request.graph, &request.patch_library)
        .map_err(AudioDspPlanError::from_issues)?;
    let graph = lower_graph_for_execution(&expanded_graph);
    let plan = build_audio_dsp_plan_from_execution_plan(&graph, &execution_plan, options)?;
    Ok((plan, graph))
}

fn validate_audio_dsp_request_schema_versions_current(
    request: &ProjectRequestCurrent,
) -> Result<(), AudioDspPlanError> {
    let mut issues = Vec::new();
    if let Some(document) = &request.document {
        if let Some(issue) = schema_version_issue("project", Some(document.schema_version.as_str()))
        {
            issues.push(issue);
        }
    }
    if let Some(issue) = schema_version_issue("graph", Some(request.graph.schema_version.as_str()))
    {
        issues.push(issue);
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(AudioDspPlanError::InvalidProject {
            issues: issues.into_boxed_slice(),
        })
    }
}

#[allow(dead_code)]
pub(crate) fn build_audio_dsp_plan(
    graph: &GraphDocument,
    registry: &NodeRegistry,
    options: AudioDspPlanOptions,
) -> Result<AudioDspPlan, AudioDspPlanError> {
    validate_audio_dsp_plan_options(options)?;

    validate_project(graph, registry).map_err(AudioDspPlanError::from_project_validation_report)?;
    let execution_plan =
        build_execution_plan(graph, registry).map_err(AudioDspPlanError::from_plan_error)?;
    build_audio_dsp_plan_from_execution_plan(graph, &execution_plan, options)
}

fn build_audio_dsp_plan_from_execution_plan(
    graph: &GraphDocument,
    execution_plan: &ExecutionPlan,
    options: AudioDspPlanOptions,
) -> Result<AudioDspPlan, AudioDspPlanError> {
    let nodes_by_id = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let audio_node_ids = execution_plan
        .nodes
        .iter()
        .filter(|node| node.execution_model == ExecutionModel::AudioBlock)
        .map(|node| node.node_id.as_str())
        .collect::<Vec<_>>();
    let audio_node_set = audio_node_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();

    reject_signal_ports_outside_audio_block(graph, &audio_node_set)?;
    let endpoint_nodes = audio_endpoint_plan_nodes(graph);
    let endpoints = audio_endpoints(graph, &endpoint_nodes);
    let clock_domains = audio_clock_domains(&endpoint_nodes, options.sample_rate);
    let partitions = audio_graph_partitions(graph, &endpoint_nodes, &audio_node_set);
    let bridge_plans = audio_clock_bridge_plans(graph, &endpoint_nodes)?;

    let mut buffers = Vec::new();
    let mut buffer_by_output = BTreeMap::new();
    for node_id in &audio_node_ids {
        let node = nodes_by_id
            .get(node_id)
            .expect("execution plan should only contain graph nodes");
        for port in node
            .ports
            .iter()
            .filter(|port| is_audio_signal_output(port))
        {
            let buffer_id = format!("audio_buffer_{}", buffers.len());
            buffer_by_output.insert((node.id.as_str(), port.id.as_str()), buffer_id.clone());
            buffers.push(AudioDspBuffer {
                id: buffer_id,
                producer_node_id: node.id.clone(),
                producer_port_id: port.id.clone(),
                sample_format: DEFAULT_SAMPLE_FORMAT.to_owned(),
                channels: 1,
            });
        }
    }

    let mut signal_edges = Vec::new();
    for edge in graph
        .edges
        .iter()
        .filter(|edge| is_audio_signal_edge(edge, graph))
    {
        let buffer_id = buffer_by_output
            .get(&(edge.from.node.as_str(), edge.from.port.as_str()))
            .cloned()
            .expect("validated audio signal edge should have a producer output buffer");
        signal_edges.push(AudioDspPlanEdge {
            from_node: edge.from.node.clone(),
            from_port: edge.from.port.clone(),
            to_node: edge.to.node.clone(),
            to_port: edge.to.port.clone(),
            buffer_id,
        });
    }

    let nodes = audio_node_ids
        .iter()
        .enumerate()
        .map(|(order, node_id)| {
            let node = nodes_by_id
                .get(node_id)
                .expect("execution plan should only contain graph nodes");
            audio_plan_node(node, order, &signal_edges, &buffer_by_output, graph)
        })
        .collect();

    Ok(AudioDspPlan {
        graph_id: graph.id.clone(),
        graph_revision: graph.revision.clone(),
        block_size: options.block_size,
        sample_rate: options.sample_rate,
        endpoints,
        clock_domains,
        partitions,
        bridge_plans,
        nodes,
        edges: signal_edges,
        buffers,
    })
}

fn lower_graph_for_execution(graph: &crate::GraphDocumentCurrent) -> GraphDocument {
    let nodes = graph
        .nodes
        .iter()
        .filter_map(|node| crate::session::lower_graph_node_for_execution(node, &node.id))
        .collect::<Vec<_>>();
    let executable_node_ids = nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();
    GraphDocument {
        schema: "skenion.graph".to_owned(),
        schema_version: "0.1.0".to_owned(),
        id: graph.id.clone(),
        revision: graph.revision.clone(),
        nodes,
        edges: graph
            .edges
            .iter()
            .filter(|edge| {
                executable_node_ids.contains(&edge.source.node_id)
                    && executable_node_ids.contains(&edge.target.node_id)
            })
            .map(crate::session::lower_edge_for_execution)
            .collect(),
    }
}

fn validate_audio_dsp_plan_options(options: AudioDspPlanOptions) -> Result<(), AudioDspPlanError> {
    if options.block_size == 0 {
        return Err(AudioDspPlanError::InvalidBlockSize {
            issue: Box::new(RuntimeIssue::structured_error(
                "audio-dsp.invalid-block-size",
                "audio dsp block size must be greater than zero",
                json!({ "blockSize": options.block_size }),
            )),
        });
    }
    if options.sample_rate == 0 {
        return Err(AudioDspPlanError::InvalidSampleRate {
            issue: Box::new(RuntimeIssue::structured_error(
                "audio-dsp.invalid-sample-rate",
                "audio dsp sample rate must be greater than zero",
                json!({ "sampleRate": options.sample_rate }),
            )),
        });
    }
    Ok(())
}

fn reject_signal_ports_outside_audio_block(
    graph: &GraphDocument,
    audio_node_set: &std::collections::HashSet<&str>,
) -> Result<(), AudioDspPlanError> {
    for node in &graph.nodes {
        if audio_node_set.contains(node.id.as_str()) {
            continue;
        }
        if let Some(port) = node.ports.iter().find(|port| is_audio_signal_port(port)) {
            return Err(AudioDspPlanError::SignalPortOutsideAudioBlock {
                node_id: node.id.clone(),
                port_id: port.id.clone(),
                issue: Box::new(RuntimeIssue::structured_error(
                    "audio-dsp.signal-port-outside-audio-block",
                    format!(
                        "audio signal port {}.{} is not an audio_block node",
                        node.id, port.id
                    ),
                    json!({
                        "nodeId": node.id.as_str(),
                        "portId": port.id.as_str(),
                    }),
                )),
            });
        }
    }
    Ok(())
}

fn audio_plan_node(
    node: &GraphNode,
    order: usize,
    signal_edges: &[AudioDspPlanEdge],
    buffer_by_output: &BTreeMap<(&str, &str), String>,
    graph: &GraphDocument,
) -> AudioDspPlanNode {
    let signal_inputs = signal_edges
        .iter()
        .filter(|edge| edge.to_node == node.id)
        .map(|edge| AudioDspSignalInput {
            port_id: edge.to_port.clone(),
            source_node_id: edge.from_node.clone(),
            source_port_id: edge.from_port.clone(),
            buffer_id: edge.buffer_id.clone(),
        })
        .collect();
    let signal_outputs = node
        .ports
        .iter()
        .filter(|port| is_audio_signal_output(port))
        .map(|port| AudioDspSignalOutput {
            port_id: port.id.clone(),
            buffer_id: buffer_by_output
                .get(&(node.id.as_str(), port.id.as_str()))
                .expect("audio output should have an allocated buffer")
                .clone(),
        })
        .collect();
    let control_inputs = node
        .ports
        .iter()
        .filter(|port| port.direction == PortDirection::Input && !is_audio_signal_port(port))
        .map(|port| {
            let source = graph
                .edges
                .iter()
                .find(|edge| edge.to.node == node.id && edge.to.port == port.id);
            AudioDspControlInput {
                port_id: port.id.clone(),
                data_kind: port.data_type.data_kind.clone(),
                source_node_id: source.map(|edge| edge.from.node.clone()),
                source_port_id: source.map(|edge| edge.from.port.clone()),
            }
        })
        .collect();

    AudioDspPlanNode {
        node_id: node.id.clone(),
        kind: node.kind.clone(),
        kind_version: node.kind_version.clone(),
        order,
        params: node.params.clone(),
        signal_inputs,
        control_inputs,
        signal_outputs,
    }
}
