use std::collections::BTreeMap;
use std::f32::consts::TAU;

use serde_json::{Map, Value, json};

use super::values::{control_input_f32_from_params, param_f32};
use super::{
    AUDIO_OUTPUT_KIND, AudioDspPlan, AudioDspPlanNode, AudioRealtimeDspError,
    AudioRealtimeDspOptions, build_audio_dsp_plan, build_audio_dsp_plan_with_graph_current,
};
use crate::{GraphDocument, NodeRegistry, ProjectRequestCurrent, RuntimeIssue};

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
        request: &ProjectRequestCurrent,
        options: AudioRealtimeDspOptions,
    ) -> Result<Self, AudioRealtimeDspError> {
        if options.channels == 0 {
            return Err(AudioRealtimeDspError::InvalidChannelCount {
                issue: invalid_channel_count_issue(),
            });
        }
        let (plan, graph) = build_audio_dsp_plan_with_graph_current(request, options.plan)?;
        Self::from_plan_and_graph(&graph, plan, options.channels)
    }

    #[allow(dead_code)]
    pub(crate) fn new_from_graph(
        graph: &GraphDocument,
        registry: &NodeRegistry,
        options: AudioRealtimeDspOptions,
    ) -> Result<Self, AudioRealtimeDspError> {
        if options.channels == 0 {
            return Err(AudioRealtimeDspError::InvalidChannelCount {
                issue: invalid_channel_count_issue(),
            });
        }
        let plan = build_audio_dsp_plan(graph, registry, options.plan)?;
        Self::from_plan_and_graph(graph, plan, options.channels)
    }

    fn from_plan_and_graph(
        graph: &GraphDocument,
        plan: AudioDspPlan,
        channels: usize,
    ) -> Result<Self, AudioRealtimeDspError> {
        let output_nodes = plan
            .nodes
            .iter()
            .filter(|node| node.kind == AUDIO_OUTPUT_KIND)
            .collect::<Vec<_>>();
        if output_nodes.len() != 1 {
            return Err(AudioRealtimeDspError::OutputCount {
                count: output_nodes.len(),
                issue: Box::new(RuntimeIssue::structured_error(
                    "audio-dsp.realtime-output-count",
                    format!(
                        "audio realtime dsp graph must contain exactly one object.core.audio.output node, found {}",
                        output_nodes.len()
                    ),
                    json!({ "count": output_nodes.len() }),
                )),
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
                issue: Box::new(RuntimeIssue::structured_error(
                    "audio-dsp.realtime-unsupported-node-kind",
                    format!(
                        "audio realtime dsp node {} uses unsupported kind {}",
                        node.node_id, node.kind
                    ),
                    json!({
                        "nodeId": node.node_id.as_str(),
                        "kind": node.kind.as_str(),
                    }),
                )),
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
            channels,
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

fn invalid_channel_count_issue() -> Box<RuntimeIssue> {
    Box::new(RuntimeIssue::structured_error(
        "audio-dsp.invalid-realtime-channel-count",
        "audio realtime dsp output channel count must be greater than zero",
        json!({ "channels": 0 }),
    ))
}

fn matches_realtime_kind(kind: &str) -> bool {
    matches!(
        kind,
        "object.core.audio.sig"
            | "object.core.audio.osc"
            | "object.core.audio.operator.mul"
            | AUDIO_OUTPUT_KIND
    )
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
        "object.core.audio.sig" => AudioRealtimeNodeKind::Sig {
            value: param_f32(&node.params, "value", 0.0),
        },
        "object.core.audio.osc" => AudioRealtimeNodeKind::Osc {
            frequency: control_input_f32_from_params(node, "frequency", node_params_by_id)
                .unwrap_or_else(|| param_f32(&node.params, "frequency", 440.0)),
        },
        "object.core.audio.operator.mul" => AudioRealtimeNodeKind::Mul,
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
