use std::collections::{BTreeMap, HashMap};
use std::f32::consts::TAU;

use serde_json::json;

use super::values::param_f32;
use super::{
    AudioDspBlockReport, AudioDspPlan, AudioDspPlanNode, AudioDspRenderedBuffer, AudioDspSnapshot,
    AudioOfflineDspError, AudioOfflineDspOptions, AudioOfflineDspReport, build_audio_dsp_plan,
    build_audio_dsp_plan_with_graph_current,
};
use crate::{GraphDocument, GraphNode, NodeRegistry, ProjectRequestCurrent, RuntimeIssue};

pub fn run_offline_audio_dsp_current(
    request: &ProjectRequestCurrent,
    options: AudioOfflineDspOptions,
) -> Result<AudioOfflineDspReport, AudioOfflineDspError> {
    validate_options(options)?;
    let (plan, graph) = build_audio_dsp_plan_with_graph_current(request, options.plan)?;
    run_with_plan(&graph, plan, options)
}

#[allow(dead_code)]
pub(crate) fn run_offline_audio_dsp(
    graph: &GraphDocument,
    registry: &NodeRegistry,
    options: AudioOfflineDspOptions,
) -> Result<AudioOfflineDspReport, AudioOfflineDspError> {
    validate_options(options)?;

    let plan = build_audio_dsp_plan(graph, registry, options.plan)?;
    run_with_plan(graph, plan, options)
}

fn validate_options(options: AudioOfflineDspOptions) -> Result<(), AudioOfflineDspError> {
    if options.blocks == 0 {
        return Err(AudioOfflineDspError::InvalidBlockCount {
            issue: Box::new(RuntimeIssue::structured_error(
                "audio-dsp.invalid-offline-block-count",
                "audio offline dsp block count must be greater than zero",
                json!({ "blocks": options.blocks }),
            )),
        });
    }
    Ok(())
}

fn run_with_plan(
    graph: &GraphDocument,
    plan: AudioDspPlan,
    options: AudioOfflineDspOptions,
) -> Result<AudioOfflineDspReport, AudioOfflineDspError> {
    let block_len = plan.block_size as usize;
    let nodes_by_id = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut oscillator_phase_by_node = BTreeMap::<String, f32>::new();
    let mut block_reports = Vec::new();
    let mut snapshots = Vec::new();

    for block_index in 0..options.blocks {
        let mut buffers = plan
            .buffers
            .iter()
            .map(|buffer| (buffer.id.clone(), vec![0.0; block_len]))
            .collect::<BTreeMap<_, _>>();

        for node in &plan.nodes {
            match node.kind.as_str() {
                "object.core.audio.sig" => render_sig(node, &mut buffers, block_len),
                "object.core.audio.osc" => render_osc(
                    node,
                    &nodes_by_id,
                    &mut oscillator_phase_by_node,
                    &mut buffers,
                    block_len,
                    plan.sample_rate,
                ),
                "object.core.audio.operator.mul" => render_mul(node, &mut buffers, block_len),
                "object.core.audio.snapshot" => {
                    snapshots.push(snapshot_signal(node, &buffers, block_index));
                }
                _ => {
                    return Err(AudioOfflineDspError::UnsupportedNodeKind {
                        node_id: node.node_id.clone(),
                        kind: node.kind.clone(),
                        issue: Box::new(RuntimeIssue::structured_error(
                            "audio-dsp.offline-unsupported-node-kind",
                            format!(
                                "offline audio dsp node {} uses unsupported kind {}",
                                node.node_id, node.kind
                            ),
                            json!({
                                "nodeId": node.node_id.as_str(),
                                "kind": node.kind.as_str(),
                            }),
                        )),
                    });
                }
            }
        }

        let rendered_buffers = plan
            .buffers
            .iter()
            .map(|buffer| AudioDspRenderedBuffer {
                buffer_id: buffer.id.clone(),
                producer_node_id: buffer.producer_node_id.clone(),
                producer_port_id: buffer.producer_port_id.clone(),
                samples: buffers
                    .get(&buffer.id)
                    .expect("allocated dsp buffer should exist for every plan buffer")
                    .clone(),
            })
            .collect();
        block_reports.push(AudioDspBlockReport {
            index: block_index,
            buffers: rendered_buffers,
        });
    }

    Ok(AudioOfflineDspReport {
        graph_id: plan.graph_id,
        graph_revision: plan.graph_revision,
        block_size: plan.block_size,
        sample_rate: plan.sample_rate,
        blocks: block_reports,
        snapshots,
    })
}

fn render_sig(node: &AudioDspPlanNode, buffers: &mut BTreeMap<String, Vec<f32>>, block_len: usize) {
    let value = param_f32(&node.params, "value", 0.0);
    write_signal_output(node, buffers, vec![value; block_len]);
}

fn render_osc(
    node: &AudioDspPlanNode,
    nodes_by_id: &HashMap<&str, &GraphNode>,
    phase_by_node: &mut BTreeMap<String, f32>,
    buffers: &mut BTreeMap<String, Vec<f32>>,
    block_len: usize,
    sample_rate: u32,
) {
    let frequency = control_input_f32(node, "frequency", nodes_by_id)
        .unwrap_or_else(|| param_f32(&node.params, "frequency", 440.0));
    let phase = phase_by_node.entry(node.node_id.clone()).or_insert(0.0);
    let increment = frequency / sample_rate as f32;
    let mut samples = Vec::with_capacity(block_len);
    for _ in 0..block_len {
        samples.push((*phase * TAU).sin());
        *phase = (*phase + increment).rem_euclid(1.0);
    }
    write_signal_output(node, buffers, samples);
}

fn render_mul(node: &AudioDspPlanNode, buffers: &mut BTreeMap<String, Vec<f32>>, block_len: usize) {
    let left = signal_input_block(node, "left", buffers, block_len);
    let right = signal_input_block(node, "right", buffers, block_len);
    let samples = left
        .iter()
        .zip(right.iter())
        .map(|(left, right)| left * right)
        .collect();
    write_signal_output(node, buffers, samples);
}

fn snapshot_signal(
    node: &AudioDspPlanNode,
    buffers: &BTreeMap<String, Vec<f32>>,
    block_index: u32,
) -> AudioDspSnapshot {
    let value = signal_input_block(node, "signal", buffers, 1)
        .first()
        .copied()
        .unwrap_or(0.0);
    AudioDspSnapshot {
        block_index,
        node_id: node.node_id.clone(),
        port_id: "value".to_owned(),
        value,
    }
}

fn write_signal_output(
    node: &AudioDspPlanNode,
    buffers: &mut BTreeMap<String, Vec<f32>>,
    samples: Vec<f32>,
) {
    let output = node
        .signal_outputs
        .first()
        .expect("offline dsp signal node should expose a signal output");
    let buffer = buffers
        .get_mut(&output.buffer_id)
        .expect("offline dsp signal output should have an allocated buffer");
    *buffer = samples;
}

fn signal_input_block(
    node: &AudioDspPlanNode,
    port_id: &str,
    buffers: &BTreeMap<String, Vec<f32>>,
    block_len: usize,
) -> Vec<f32> {
    node.signal_inputs
        .iter()
        .find(|input| input.port_id == port_id)
        .and_then(|input| buffers.get(&input.buffer_id))
        .cloned()
        .unwrap_or_else(|| vec![0.0; block_len])
}

fn control_input_f32(
    node: &AudioDspPlanNode,
    port_id: &str,
    nodes_by_id: &HashMap<&str, &GraphNode>,
) -> Option<f32> {
    node.control_inputs
        .iter()
        .find(|input| input.port_id == port_id)
        .and_then(|input| input.source_node_id.as_deref())
        .and_then(|node_id| nodes_by_id.get(node_id))
        .map(|node| param_f32(&node.params, "value", 0.0))
}
