use std::collections::{BTreeMap, HashMap};
use std::f32::consts::TAU;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{
    AudioClockBridgeMethod, AudioClockBridgePlan, AudioClockDomain, AudioClockDomainAuthority,
    AudioEndpoint, AudioEndpointDirection, AudioGraphPartition, DataFlow, Edge, ExecutionModel,
    GraphDocument, GraphNode, NodeRegistry, PlanError, Port, PortDirection, build_execution_plan,
    plan_audio_clock_bridge, validate_project,
};

const AUDIO_SIGNAL_KIND: &str = "signal.audio";
const AUDIO_INPUT_KIND: &str = "audio.input";
const AUDIO_OUTPUT_KIND: &str = "audio.output";
const AUDIO_CLOCK_BRIDGE_KIND: &str = "audio.clock-bridge";
const AUDIO_RESAMPLE_KIND: &str = "audio.resample";
const DEFAULT_BLOCK_SIZE: u32 = 64;
const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_SAMPLE_FORMAT: &str = "f32";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioDspPlanOptions {
    pub block_size: u32,
    pub sample_rate: u32,
}

impl Default for AudioDspPlanOptions {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE,
            sample_rate: DEFAULT_SAMPLE_RATE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDspPlan {
    pub graph_id: String,
    pub graph_revision: String,
    pub block_size: u32,
    pub sample_rate: u32,
    pub endpoints: Vec<AudioEndpoint>,
    pub clock_domains: Vec<AudioClockDomain>,
    pub partitions: Vec<AudioGraphPartition>,
    pub bridge_plans: Vec<AudioClockBridgePlan>,
    pub nodes: Vec<AudioDspPlanNode>,
    pub edges: Vec<AudioDspPlanEdge>,
    pub buffers: Vec<AudioDspBuffer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioEndpointPlanNode {
    pub node_id: String,
    pub kind: String,
    pub clock_domain_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDspPlanNode {
    pub node_id: String,
    pub kind: String,
    pub kind_version: String,
    pub order: usize,
    pub params: Map<String, Value>,
    pub signal_inputs: Vec<AudioDspSignalInput>,
    pub control_inputs: Vec<AudioDspControlInput>,
    pub signal_outputs: Vec<AudioDspSignalOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDspSignalInput {
    pub port_id: String,
    pub source_node_id: String,
    pub source_port_id: String,
    pub buffer_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDspControlInput {
    pub port_id: String,
    pub data_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_port_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDspSignalOutput {
    pub port_id: String,
    pub buffer_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDspPlanEdge {
    pub from_node: String,
    pub from_port: String,
    pub to_node: String,
    pub to_port: String,
    pub buffer_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDspBuffer {
    pub id: String,
    pub producer_node_id: String,
    pub producer_port_id: String,
    pub sample_format: String,
    pub channels: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioOfflineDspOptions {
    pub blocks: u32,
    pub plan: AudioDspPlanOptions,
}

impl Default for AudioOfflineDspOptions {
    fn default() -> Self {
        Self {
            blocks: 1,
            plan: AudioDspPlanOptions::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioOfflineDspReport {
    pub graph_id: String,
    pub graph_revision: String,
    pub block_size: u32,
    pub sample_rate: u32,
    pub blocks: Vec<AudioDspBlockReport>,
    pub snapshots: Vec<AudioDspSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDspBlockReport {
    pub index: u32,
    pub buffers: Vec<AudioDspRenderedBuffer>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDspRenderedBuffer {
    pub buffer_id: String,
    pub producer_node_id: String,
    pub producer_port_id: String,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDspSnapshot {
    pub block_index: u32,
    pub node_id: String,
    pub port_id: String,
    pub value: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioRealtimeDspOptions {
    pub plan: AudioDspPlanOptions,
    pub channels: usize,
}

impl Default for AudioRealtimeDspOptions {
    fn default() -> Self {
        Self {
            plan: AudioDspPlanOptions::default(),
            channels: 2,
        }
    }
}

#[derive(Debug, Error)]
pub enum AudioDspPlanError {
    #[error("{0}")]
    InvalidProject(#[from] crate::ProjectValidationReport),
    #[error("{0}")]
    Plan(#[from] PlanError),
    #[error("audio dsp block size must be greater than zero")]
    InvalidBlockSize,
    #[error("audio dsp sample rate must be greater than zero")]
    InvalidSampleRate,
    #[error("audio signal port {node_id}.{port_id} is not an audio_block node")]
    SignalPortOutsideAudioBlock { node_id: String, port_id: String },
    #[error(
        "audio signal route from {source_node_id} domain {source_clock_domain_id} to {target_node_id} domain {target_clock_domain_id} requires audio.clock-bridge or audio.resample"
    )]
    ClockDomainCrossingRequiresBridge {
        source_node_id: String,
        target_node_id: String,
        source_clock_domain_id: String,
        target_clock_domain_id: String,
    },
}

#[derive(Debug, Error)]
pub enum AudioOfflineDspError {
    #[error("{0}")]
    Plan(#[from] AudioDspPlanError),
    #[error("audio offline dsp block count must be greater than zero")]
    InvalidBlockCount,
    #[error("offline audio dsp node {node_id} uses unsupported kind {kind}")]
    UnsupportedNodeKind { node_id: String, kind: String },
}

#[derive(Debug, Error)]
pub enum AudioRealtimeDspError {
    #[error("{0}")]
    Plan(#[from] AudioDspPlanError),
    #[error("audio realtime dsp output channel count must be greater than zero")]
    InvalidChannelCount,
    #[error("audio realtime dsp graph must contain exactly one audio.output node, found {count}")]
    OutputCount { count: usize },
    #[error("audio realtime dsp node {node_id} uses unsupported kind {kind}")]
    UnsupportedNodeKind { node_id: String, kind: String },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AudioRealtimeNodeKind {
    Sig { value: f32 },
    Osc { frequency: f32 },
    Mul,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct AudioRealtimeNode {
    kind: AudioRealtimeNodeKind,
    output_buffer: Option<usize>,
    left_buffer: Option<usize>,
    right_buffer: Option<usize>,
    phase: f32,
}

#[derive(Debug, Clone)]
pub struct AudioRealtimeDspExecutor {
    plan: AudioDspPlan,
    channels: usize,
    block_len: usize,
    output_node_index: usize,
    nodes: Vec<AudioRealtimeNode>,
    buffers: Vec<Vec<f32>>,
    scratch: Vec<f32>,
}

impl AudioRealtimeDspExecutor {
    pub fn new(
        graph: &GraphDocument,
        registry: &NodeRegistry,
        options: AudioRealtimeDspOptions,
    ) -> Result<Self, AudioRealtimeDspError> {
        if options.channels == 0 {
            return Err(AudioRealtimeDspError::InvalidChannelCount);
        }
        let plan = build_audio_dsp_plan(graph, registry, options.plan)?;
        let output_nodes = plan
            .nodes
            .iter()
            .filter(|node| node.kind == AUDIO_OUTPUT_KIND)
            .collect::<Vec<_>>();
        if output_nodes.len() != 1 {
            return Err(AudioRealtimeDspError::OutputCount {
                count: output_nodes.len(),
            });
        }
        if let Some(node) = plan
            .nodes
            .iter()
            .find(|node| !matches_realtime_kind(node.kind.as_str()))
        {
            return Err(AudioRealtimeDspError::UnsupportedNodeKind {
                node_id: node.node_id.clone(),
                kind: node.kind.clone(),
            });
        }

        let node_params_by_id = graph
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node.params.clone()))
            .collect::<BTreeMap<_, _>>();
        let buffer_index_by_id = plan
            .buffers
            .iter()
            .enumerate()
            .map(|(index, buffer)| (buffer.id.clone(), index))
            .collect::<BTreeMap<_, _>>();
        let block_len = plan.block_size as usize;
        let buffers = plan
            .buffers
            .iter()
            .map(|_| vec![0.0; block_len])
            .collect::<Vec<_>>();
        let nodes = plan
            .nodes
            .iter()
            .map(|node| realtime_node(node, &buffer_index_by_id, &node_params_by_id))
            .collect::<Vec<_>>();
        let output_node_index = plan
            .nodes
            .iter()
            .position(|node| node.kind == AUDIO_OUTPUT_KIND)
            .expect("realtime output count was already validated");

        Ok(Self {
            plan,
            channels: options.channels,
            block_len,
            output_node_index,
            nodes,
            buffers,
            scratch: vec![0.0; block_len],
        })
    }

    pub fn plan(&self) -> &AudioDspPlan {
        &self.plan
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn process_interleaved_output(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        if output.is_empty() {
            return;
        }
        let total_frames = output.len() / self.channels;
        let mut frame_offset = 0;
        while frame_offset < total_frames {
            self.process_block();
            let frames_to_copy = self.block_len.min(total_frames - frame_offset);
            self.copy_output_frames(output, frame_offset, frames_to_copy);
            frame_offset += frames_to_copy;
        }
    }

    fn process_block(&mut self) {
        for node_index in 0..self.nodes.len() {
            match self.nodes[node_index].kind {
                AudioRealtimeNodeKind::Sig { value } => self.render_sig(node_index, value),
                AudioRealtimeNodeKind::Osc { frequency } => {
                    self.render_osc(node_index, frequency);
                }
                AudioRealtimeNodeKind::Mul => self.render_mul(node_index),
                AudioRealtimeNodeKind::Output => {}
            }
        }
    }

    fn render_sig(&mut self, node_index: usize, value: f32) {
        if let Some(buffer) = self.signal_output_buffer_mut(node_index) {
            buffer.fill(value);
        }
    }

    fn render_osc(&mut self, node_index: usize, frequency: f32) {
        let increment = frequency / self.plan.sample_rate as f32;
        let mut phase_value = self.nodes[node_index].phase;
        let samples = self.scratch.as_mut_slice();
        for sample in samples.iter_mut().take(self.block_len) {
            *sample = (phase_value * TAU).sin();
            phase_value = (phase_value + increment).rem_euclid(1.0);
        }
        self.nodes[node_index].phase = phase_value;
        self.copy_scratch_to_output(node_index);
    }

    fn render_mul(&mut self, node_index: usize) {
        let left = self.nodes[node_index].left_buffer;
        let right = self.nodes[node_index].right_buffer;
        for index in 0..self.block_len {
            let sample = self.sample(left, index) * self.sample(right, index);
            self.scratch[index] = sample;
        }
        self.copy_scratch_to_output(node_index);
    }

    fn copy_scratch_to_output(&mut self, node_index: usize) {
        if let Some(output_index) = self.nodes[node_index].output_buffer {
            self.buffers[output_index].copy_from_slice(&self.scratch);
        }
    }

    fn copy_output_frames(
        &mut self,
        output: &mut [f32],
        frame_offset: usize,
        frames_to_copy: usize,
    ) {
        let output_node = self.nodes[self.output_node_index];
        let left = output_node.left_buffer;
        let right = output_node.right_buffer;
        for local_frame in 0..frames_to_copy {
            let output_frame = frame_offset + local_frame;
            let output_base = output_frame * self.channels;
            output[output_base] = self.sample(left, local_frame);
            if self.channels > 1 {
                output[output_base + 1] = self.sample(right, local_frame);
            }
        }
    }

    fn signal_output_buffer_mut(&mut self, node_index: usize) -> Option<&mut Vec<f32>> {
        self.nodes[node_index]
            .output_buffer
            .map(|output_buffer| &mut self.buffers[output_buffer])
    }

    fn sample(&self, buffer_id: Option<usize>, index: usize) -> f32 {
        buffer_id
            .and_then(|id| self.buffers.get(id))
            .and_then(|buffer| buffer.get(index))
            .copied()
            .unwrap_or(0.0)
    }
}

pub fn build_audio_dsp_plan(
    graph: &GraphDocument,
    registry: &NodeRegistry,
    options: AudioDspPlanOptions,
) -> Result<AudioDspPlan, AudioDspPlanError> {
    if options.block_size == 0 {
        return Err(AudioDspPlanError::InvalidBlockSize);
    }
    if options.sample_rate == 0 {
        return Err(AudioDspPlanError::InvalidSampleRate);
    }

    validate_project(graph, registry)?;
    let execution_plan = build_execution_plan(graph, registry)?;
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

pub fn run_offline_audio_dsp(
    graph: &GraphDocument,
    registry: &NodeRegistry,
    options: AudioOfflineDspOptions,
) -> Result<AudioOfflineDspReport, AudioOfflineDspError> {
    if options.blocks == 0 {
        return Err(AudioOfflineDspError::InvalidBlockCount);
    }

    let plan = build_audio_dsp_plan(graph, registry, options.plan)?;
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
                "audio.sig" => render_sig(node, &mut buffers, block_len),
                "audio.osc" => render_osc(
                    node,
                    &nodes_by_id,
                    &mut oscillator_phase_by_node,
                    &mut buffers,
                    block_len,
                    plan.sample_rate,
                ),
                "audio.operator.mul" => render_mul(node, &mut buffers, block_len),
                "audio.snapshot" => {
                    snapshots.push(snapshot_signal(node, &buffers, block_index));
                }
                _ => {
                    return Err(AudioOfflineDspError::UnsupportedNodeKind {
                        node_id: node.node_id.clone(),
                        kind: node.kind.clone(),
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

fn control_input_f32_from_params(
    node: &AudioDspPlanNode,
    port_id: &str,
    node_params_by_id: &BTreeMap<String, Map<String, Value>>,
) -> Option<f32> {
    for input in &node.control_inputs {
        if input.port_id != port_id {
            continue;
        }
        let node_id = input.source_node_id.as_deref()?;
        let params = node_params_by_id.get(node_id)?;
        return Some(param_f32(params, "value", 0.0));
    }
    None
}

fn param_f32(params: &Map<String, Value>, key: &str, default: f32) -> f32 {
    params
        .get(key)
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .unwrap_or(default)
}

fn matches_realtime_kind(kind: &str) -> bool {
    matches!(
        kind,
        "audio.sig" | "audio.osc" | "audio.operator.mul" | AUDIO_OUTPUT_KIND
    )
}

fn audio_endpoint_plan_nodes(graph: &GraphDocument) -> Vec<AudioEndpointPlanNode> {
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == AUDIO_INPUT_KIND || node.kind == AUDIO_OUTPUT_KIND)
        .map(|node| AudioEndpointPlanNode {
            node_id: node.id.clone(),
            kind: node.kind.clone(),
            clock_domain_id: audio_clock_domain_id(node),
        })
        .collect()
}

fn audio_clock_domain_id(node: &GraphNode) -> String {
    node.params
        .get("clockDomain")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("endpoint:{}", node.id))
}

fn audio_endpoints(
    graph: &GraphDocument,
    endpoint_nodes: &[AudioEndpointPlanNode],
) -> Vec<AudioEndpoint> {
    endpoint_nodes
        .iter()
        .filter_map(|endpoint| {
            let node = graph
                .nodes
                .iter()
                .find(|node| node.id == endpoint.node_id)?;
            let direction = if node.kind == AUDIO_INPUT_KIND {
                AudioEndpointDirection::Input
            } else {
                AudioEndpointDirection::Output
            };
            let channel_ports = node
                .ports
                .iter()
                .filter(|port| is_audio_signal_port(port))
                .map(|port| port.id.clone())
                .collect();
            Some(AudioEndpoint {
                id: node.id.clone(),
                node_id: node.id.clone(),
                direction,
                channel_ports,
                requested_config: None,
                resolved_config: None,
                clock_domain_id: Some(endpoint.clock_domain_id.clone()),
            })
        })
        .collect()
}

fn audio_clock_domains(
    endpoint_nodes: &[AudioEndpointPlanNode],
    sample_rate: u32,
) -> Vec<AudioClockDomain> {
    let mut domains = BTreeMap::<String, Vec<String>>::new();
    for endpoint in endpoint_nodes {
        domains
            .entry(endpoint.clock_domain_id.clone())
            .or_default()
            .push(endpoint.node_id.clone());
    }
    domains
        .into_iter()
        .map(|(id, endpoint_ids)| AudioClockDomain {
            id: id.clone(),
            authority: if id.starts_with("endpoint:") {
                AudioClockDomainAuthority::Unavailable
            } else {
                AudioClockDomainAuthority::UserConfigured
            },
            source: "runtime.audio-domain-planner.v0".to_owned(),
            sample_rate: Some(sample_rate),
            drift_compensated: None,
            shared_with: Some(endpoint_ids),
        })
        .collect()
}

fn audio_graph_partitions(
    graph: &GraphDocument,
    endpoint_nodes: &[AudioEndpointPlanNode],
    audio_node_set: &std::collections::HashSet<&str>,
) -> Vec<AudioGraphPartition> {
    let mut nodes_by_domain = BTreeMap::<String, Vec<String>>::new();
    for endpoint in endpoint_nodes {
        nodes_by_domain
            .entry(endpoint.clock_domain_id.clone())
            .or_default()
            .push(endpoint.node_id.clone());
    }
    if endpoint_nodes.is_empty() {
        return Vec::new();
    }
    let default_domain = endpoint_nodes
        .iter()
        .find(|endpoint| endpoint.kind == AUDIO_OUTPUT_KIND)
        .or_else(|| endpoint_nodes.first())
        .map(|endpoint| endpoint.clock_domain_id.clone())
        .expect("endpoint_nodes is not empty");
    for node in &graph.nodes {
        if !audio_node_set.contains(node.id.as_str()) || is_audio_endpoint_kind(&node.kind) {
            continue;
        }
        nodes_by_domain
            .entry(default_domain.clone())
            .or_default()
            .push(node.id.clone());
    }
    nodes_by_domain
        .into_iter()
        .map(|(clock_domain_id, node_ids)| {
            let endpoint_ids = endpoint_nodes
                .iter()
                .filter(|endpoint| endpoint.clock_domain_id == clock_domain_id)
                .map(|endpoint| endpoint.node_id.clone())
                .collect::<Vec<_>>();
            AudioGraphPartition {
                id: format!("partition:{clock_domain_id}"),
                clock_domain_id,
                endpoint_ids,
                node_ids,
            }
        })
        .collect()
}

fn audio_clock_bridge_plans(
    graph: &GraphDocument,
    endpoint_nodes: &[AudioEndpointPlanNode],
) -> Result<Vec<AudioClockBridgePlan>, AudioDspPlanError> {
    let nodes_by_id = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut plans = Vec::new();

    for source in endpoint_nodes
        .iter()
        .filter(|endpoint| endpoint.kind == AUDIO_INPUT_KIND)
    {
        for output in endpoint_nodes
            .iter()
            .filter(|endpoint| endpoint.kind == AUDIO_OUTPUT_KIND)
        {
            let Some(route) =
                find_audio_route_to_output(graph, &nodes_by_id, &source.node_id, &output.node_id)
            else {
                continue;
            };
            let bridge_node_id = route.explicit_bridge_node_id.as_deref();
            let source_domain = endpoint_domain(source);
            let target_domain = endpoint_domain(output);
            let mut plan = plan_audio_clock_bridge(&source_domain, &target_domain, bridge_node_id);
            if matches!(
                route.explicit_bridge_kind.as_deref(),
                Some(AUDIO_RESAMPLE_KIND)
            ) {
                plan.method = AudioClockBridgeMethod::Resample;
            }
            if plan.method == AudioClockBridgeMethod::Invalid {
                return Err(AudioDspPlanError::ClockDomainCrossingRequiresBridge {
                    source_node_id: source.node_id.clone(),
                    target_node_id: output.node_id.clone(),
                    source_clock_domain_id: source.clock_domain_id.clone(),
                    target_clock_domain_id: output.clock_domain_id.clone(),
                });
            }
            plans.push(plan);
        }
    }

    Ok(plans)
}

fn endpoint_domain(endpoint: &AudioEndpointPlanNode) -> AudioClockDomain {
    AudioClockDomain {
        id: endpoint.clock_domain_id.clone(),
        authority: if endpoint.clock_domain_id.starts_with("endpoint:") {
            AudioClockDomainAuthority::Unavailable
        } else {
            AudioClockDomainAuthority::UserConfigured
        },
        source: endpoint.node_id.clone(),
        sample_rate: None,
        drift_compensated: None,
        shared_with: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AudioSignalRoute {
    explicit_bridge_node_id: Option<String>,
    explicit_bridge_kind: Option<String>,
}

fn find_audio_route_to_output(
    graph: &GraphDocument,
    nodes_by_id: &HashMap<&str, &GraphNode>,
    source_node_id: &str,
    output_node_id: &str,
) -> Option<AudioSignalRoute> {
    let mut stack = vec![(source_node_id.to_owned(), None::<(String, String)>)];
    let mut visited = std::collections::HashSet::<String>::new();
    while let Some((node_id, bridge)) = stack.pop() {
        if !visited.insert(node_id.clone()) {
            continue;
        }
        if node_id == output_node_id {
            return Some(AudioSignalRoute {
                explicit_bridge_node_id: bridge.as_ref().map(|(id, _)| id.clone()),
                explicit_bridge_kind: bridge.map(|(_, kind)| kind),
            });
        }
        for edge in graph.edges.iter().filter(|edge| edge.from.node == node_id) {
            if !is_audio_signal_edge(edge, graph) {
                continue;
            }
            let next_bridge = nodes_by_id
                .get(edge.to.node.as_str())
                .filter(|node| is_audio_clock_boundary_kind(&node.kind))
                .map(|node| (node.id.clone(), node.kind.clone()))
                .or_else(|| bridge.clone());
            stack.push((edge.to.node.clone(), next_bridge));
        }
    }
    None
}

fn is_audio_endpoint_kind(kind: &str) -> bool {
    kind == AUDIO_INPUT_KIND || kind == AUDIO_OUTPUT_KIND
}

fn is_audio_clock_boundary_kind(kind: &str) -> bool {
    kind == AUDIO_CLOCK_BRIDGE_KIND || kind == AUDIO_RESAMPLE_KIND
}

fn realtime_node(
    node: &AudioDspPlanNode,
    buffer_index_by_id: &BTreeMap<String, usize>,
    node_params_by_id: &BTreeMap<String, Map<String, Value>>,
) -> AudioRealtimeNode {
    AudioRealtimeNode {
        kind: realtime_node_kind(node, node_params_by_id),
        output_buffer: signal_output_buffer_index(node, buffer_index_by_id),
        left_buffer: signal_input_buffer_index(node, "left", buffer_index_by_id),
        right_buffer: signal_input_buffer_index(node, "right", buffer_index_by_id),
        phase: 0.0,
    }
}

fn realtime_node_kind(
    node: &AudioDspPlanNode,
    node_params_by_id: &BTreeMap<String, Map<String, Value>>,
) -> AudioRealtimeNodeKind {
    match node.kind.as_str() {
        "audio.sig" => AudioRealtimeNodeKind::Sig {
            value: param_f32(&node.params, "value", 0.0),
        },
        "audio.osc" => AudioRealtimeNodeKind::Osc {
            frequency: control_input_f32_from_params(node, "frequency", node_params_by_id)
                .unwrap_or_else(|| param_f32(&node.params, "frequency", 440.0)),
        },
        "audio.operator.mul" => AudioRealtimeNodeKind::Mul,
        _ => AudioRealtimeNodeKind::Output,
    }
}

fn signal_input_buffer_index(
    node: &AudioDspPlanNode,
    port_id: &str,
    buffer_index_by_id: &BTreeMap<String, usize>,
) -> Option<usize> {
    node.signal_inputs
        .iter()
        .find(|input| input.port_id == port_id)
        .and_then(|input| buffer_index_by_id.get(&input.buffer_id))
        .copied()
}

fn signal_output_buffer_index(
    node: &AudioDspPlanNode,
    buffer_index_by_id: &BTreeMap<String, usize>,
) -> Option<usize> {
    node.signal_outputs
        .first()
        .and_then(|output| buffer_index_by_id.get(&output.buffer_id))
        .copied()
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

fn is_audio_signal_edge(edge: &Edge, graph: &GraphDocument) -> bool {
    let Some(from) = find_port(graph, &edge.from.node, &edge.from.port) else {
        return false;
    };
    let Some(to) = find_port(graph, &edge.to.node, &edge.to.port) else {
        return false;
    };
    is_audio_signal_output(from) && is_audio_signal_input(to)
}

fn find_port<'a>(graph: &'a GraphDocument, node_id: &str, port_id: &str) -> Option<&'a Port> {
    graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .and_then(|node| node.ports.iter().find(|port| port.id == port_id))
}

fn is_audio_signal_port(port: &Port) -> bool {
    port.data_type.flow == DataFlow::Signal && port.data_type.data_kind == AUDIO_SIGNAL_KIND
}

fn is_audio_signal_input(port: &Port) -> bool {
    port.direction == PortDirection::Input && is_audio_signal_port(port)
}

fn is_audio_signal_output(port: &Port) -> bool {
    port.direction == PortDirection::Output && is_audio_signal_port(port)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::NodeDefinition;

    fn registry() -> NodeRegistry {
        let mut registry = NodeRegistry::new();
        for definition in [
            audio_source_definition("audio.osc", "frequency"),
            audio_sig_definition(),
            audio_binary_definition("audio.operator.mul"),
            audio_unary_definition("audio.operator.sqrt", "in"),
            audio_snapshot_definition(),
            audio_input_definition(),
            audio_output_definition(),
            audio_clock_boundary_definition("audio.clock-bridge"),
            audio_clock_boundary_definition("audio.resample"),
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
            audio_binary_node("mul", "audio.operator.mul"),
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
                data_kind: "number.float".to_owned(),
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

        let plan =
            build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

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

        let plan =
            build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

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

        let plan =
            build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

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

        let plan =
            build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

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
            audio_clock_boundary_node("bridge", "audio.clock-bridge"),
            audio_output_node_with_domain("output", "device:output-clock")
          ],
          "edges": [
            { "from": { "node": "input", "port": "out" }, "to": { "node": "bridge", "port": "in" } },
            { "from": { "node": "bridge", "port": "out" }, "to": { "node": "output", "port": "left" } }
          ]
        }));

        let plan =
            build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

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
            audio_clock_boundary_node("bridge", "audio.clock-bridge"),
            audio_output_node("output")
          ],
          "edges": [
            { "from": { "node": "input", "port": "out" }, "to": { "node": "bridge", "port": "in" } },
            { "from": { "node": "bridge", "port": "out" }, "to": { "node": "output", "port": "left" } }
          ]
        }));

        let plan =
            build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

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
            audio_clock_boundary_node("resample", "audio.resample"),
            audio_output_node_with_domain("output", "device:output-clock")
          ],
          "edges": [
            { "from": { "node": "input", "port": "out" }, "to": { "node": "resample", "port": "in" } },
            { "from": { "node": "resample", "port": "out" }, "to": { "node": "output", "port": "left" } }
          ]
        }));

        let plan =
            build_audio_dsp_plan(&graph, &registry(), AudioDspPlanOptions::default()).unwrap();

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
            audio_clock_boundary_node("bridge_a", "audio.clock-bridge"),
            audio_clock_boundary_node("bridge_b", "audio.clock-bridge"),
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
            Some("audio.clock-bridge")
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

        assert!(matches!(block_error, AudioDspPlanError::InvalidBlockSize));
        assert!(matches!(rate_error, AudioDspPlanError::InvalidSampleRate));
        assert!(matches!(
            project_error,
            AudioDspPlanError::InvalidProject(_)
        ));
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
          "nodes": [audio_osc_node("osc"), audio_binary_node("mul", "audio.operator.mul")],
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
            audio_binary_node("mul", "audio.operator.mul"),
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
            audio_binary_node("mul", "audio.operator.mul")
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
            audio_unary_node("sqrt", "audio.operator.sqrt")
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
            run_offline_audio_dsp(&graph, &registry(), AudioOfflineDspOptions::default())
                .unwrap_err();

        assert!(matches!(
            block_error,
            AudioOfflineDspError::InvalidBlockCount
        ));
        assert!(matches!(
            unsupported_error,
            AudioOfflineDspError::UnsupportedNodeKind { .. }
        ));
        assert!(
            unsupported_error
                .to_string()
                .contains("audio.operator.sqrt")
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
        let mut executor = AudioRealtimeDspExecutor::new(
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
            audio_binary_node("mul", "audio.operator.mul"),
            audio_output_node("out")
          ],
          "edges": [
            { "from": { "node": "left", "port": "out" }, "to": { "node": "mul", "port": "left" } },
            { "from": { "node": "right", "port": "out" }, "to": { "node": "mul", "port": "right" } },
            { "from": { "node": "mul", "port": "out" }, "to": { "node": "out", "port": "left" } },
            { "from": { "node": "mul", "port": "out" }, "to": { "node": "out", "port": "right" } }
          ]
        }));
        let mut executor = AudioRealtimeDspExecutor::new(
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
        let mut executor = AudioRealtimeDspExecutor::new(
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
            audio_unary_node("sqrt", "audio.operator.sqrt"),
            audio_output_node("out")
          ],
          "edges": [
            { "from": { "node": "sqrt", "port": "out" }, "to": { "node": "out", "port": "left" } }
          ]
        }));
        let invalid_channels = AudioRealtimeDspExecutor::new(
            &graph_without_output,
            &registry(),
            AudioRealtimeDspOptions {
                plan: AudioDspPlanOptions::default(),
                channels: 0,
            },
        )
        .unwrap_err();
        let missing_output = AudioRealtimeDspExecutor::new(
            &graph_without_output,
            &registry(),
            AudioRealtimeDspOptions::default(),
        )
        .unwrap_err();
        let duplicate_output = AudioRealtimeDspExecutor::new(
            &duplicate_outputs,
            &registry(),
            AudioRealtimeDspOptions::default(),
        )
        .unwrap_err();
        let unsupported_kind = AudioRealtimeDspExecutor::new(
            &unsupported,
            &registry(),
            AudioRealtimeDspOptions::default(),
        )
        .unwrap_err();

        assert!(matches!(
            invalid_channels,
            AudioRealtimeDspError::InvalidChannelCount
        ));
        assert!(matches!(
            missing_output,
            AudioRealtimeDspError::OutputCount { count: 0 }
        ));
        assert!(matches!(
            duplicate_output,
            AudioRealtimeDspError::OutputCount { count: 2 }
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
            kind: "audio.osc".to_owned(),
            kind_version: "0.1.0".to_owned(),
            order: 0,
            params: Map::new(),
            signal_inputs: Vec::new(),
            control_inputs: vec![
                AudioDspControlInput {
                    port_id: "other".to_owned(),
                    data_kind: "number.float".to_owned(),
                    source_node_id: None,
                    source_port_id: None,
                },
                AudioDspControlInput {
                    port_id: "frequency".to_owned(),
                    data_kind: "number.float".to_owned(),
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

    fn audio_source_definition(id: &str, input_port: &str) -> NodeDefinition {
        node_definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": id,
          "version": "0.1.0",
          "displayName": id,
          "category": "Audio",
          "ports": [
            { "id": input_port, "direction": "input", "type": { "flow": "value", "dataKind": "number.float" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
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
          "id": "audio.sig",
          "version": "0.1.0",
          "displayName": "sig~",
          "category": "Audio",
          "ports": [
            { "id": "value", "direction": "input", "type": { "flow": "value", "dataKind": "number.float" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
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
            { "id": "left", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "right", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
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
            { "id": input_port, "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
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
          "id": "audio.snapshot",
          "version": "0.1.0",
          "displayName": "snapshot~",
          "category": "Audio",
          "ports": [
            { "id": "signal", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "trigger", "direction": "input", "type": { "flow": "event", "dataKind": "message.any" }, "activation": "trigger" },
            { "id": "value", "direction": "output", "type": { "flow": "value", "dataKind": "number.float" } }
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
          "id": "audio.input",
          "version": "0.1.0",
          "displayName": "adc~",
          "category": "Audio",
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
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
          "id": "audio.output",
          "version": "0.1.0",
          "displayName": "dac~",
          "category": "Audio",
          "ports": [
            { "id": "left", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "right", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" }
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
            { "id": "in", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
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
          "id": "core.float",
          "version": "0.1.0",
          "displayName": "Float",
          "category": "Core",
          "ports": [
            { "id": "value", "direction": "output", "type": { "flow": "value", "dataKind": "number.float" } }
          ],
          "execution": { "model": "value" },
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
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
          ],
          "execution": { "model": "value" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn node_definition(value: serde_json::Value) -> NodeDefinition {
        serde_json::from_value(value).unwrap()
    }

    fn float_node(id: &str, value: f64) -> serde_json::Value {
        json!({
          "id": id,
          "kind": "core.float",
          "kindVersion": "0.1.0",
          "params": { "value": value },
          "ports": [
            { "id": "value", "direction": "output", "type": { "flow": "value", "dataKind": "number.float" } }
          ]
        })
    }

    fn audio_osc_node(id: &str) -> serde_json::Value {
        json!({
          "id": id,
          "kind": "audio.osc",
          "kindVersion": "0.1.0",
          "params": { "frequency": 440.0 },
          "ports": [
            { "id": "frequency", "direction": "input", "type": { "flow": "value", "dataKind": "number.float" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
          ]
        })
    }

    fn audio_sig_node(id: &str, value: f64) -> serde_json::Value {
        json!({
          "id": id,
          "kind": "audio.sig",
          "kindVersion": "0.1.0",
          "params": { "value": value },
          "ports": [
            { "id": "value", "direction": "input", "type": { "flow": "value", "dataKind": "number.float" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
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
            { "id": "left", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "right", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
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
            { "id": "in", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
          ]
        })
    }

    fn audio_snapshot_node(id: &str) -> serde_json::Value {
        json!({
          "id": id,
          "kind": "audio.snapshot",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "signal", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "trigger", "direction": "input", "type": { "flow": "event", "dataKind": "message.any" }, "activation": "trigger" },
            { "id": "value", "direction": "output", "type": { "flow": "value", "dataKind": "number.float" } }
          ]
        })
    }

    fn audio_output_node(id: &str) -> serde_json::Value {
        json!({
          "id": id,
          "kind": "audio.output",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "left", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "right", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" }
          ]
        })
    }

    fn audio_input_node(id: &str) -> serde_json::Value {
        json!({
          "id": id,
          "kind": "audio.input",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
          ]
        })
    }

    fn audio_input_node_with_domain(id: &str, clock_domain: &str) -> serde_json::Value {
        json!({
          "id": id,
          "kind": "audio.input",
          "kindVersion": "0.1.0",
          "params": { "clockDomain": clock_domain },
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
          ]
        })
    }

    fn audio_output_node_with_domain(id: &str, clock_domain: &str) -> serde_json::Value {
        json!({
          "id": id,
          "kind": "audio.output",
          "kindVersion": "0.1.0",
          "params": { "clockDomain": clock_domain },
          "ports": [
            { "id": "left", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "right", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" }
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
            { "id": "in", "direction": "input", "type": { "flow": "signal", "dataKind": "signal.audio" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
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
            { "id": "out", "direction": "output", "type": { "flow": "signal", "dataKind": "signal.audio" } }
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
}
