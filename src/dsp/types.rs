use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

use super::{DEFAULT_BLOCK_SIZE, DEFAULT_SAMPLE_RATE};
use crate::{
    AudioClockBridgePlan, AudioClockDomain, AudioEndpoint, AudioGraphPartition, PlanError,
    RuntimeIssue,
};

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
    #[error("audio dsp project validation failed")]
    InvalidProject { issues: Box<[RuntimeIssue]> },
    #[error("{message}")]
    Plan {
        message: String,
        issues: Box<[RuntimeIssue]>,
    },
    #[error("audio dsp block size must be greater than zero")]
    InvalidBlockSize { issue: Box<RuntimeIssue> },
    #[error("audio dsp sample rate must be greater than zero")]
    InvalidSampleRate { issue: Box<RuntimeIssue> },
    #[error("audio signal port {node_id}.{port_id} is not an audio_block node")]
    SignalPortOutsideAudioBlock {
        node_id: String,
        port_id: String,
        issue: Box<RuntimeIssue>,
    },
    #[error(
        "audio signal route from {source_node_id} domain {source_clock_domain_id} to {target_node_id} domain {target_clock_domain_id} requires object.core.audio.clock-bridge or object.core.audio.resample"
    )]
    ClockDomainCrossingRequiresBridge {
        source_node_id: String,
        target_node_id: String,
        source_clock_domain_id: String,
        target_clock_domain_id: String,
        issue: Box<RuntimeIssue>,
    },
}

impl AudioDspPlanError {
    pub fn issues(&self) -> Vec<RuntimeIssue> {
        match self {
            Self::InvalidProject { issues } | Self::Plan { issues, .. } => issues.to_vec(),
            Self::InvalidBlockSize { issue }
            | Self::InvalidSampleRate { issue }
            | Self::SignalPortOutsideAudioBlock { issue, .. }
            | Self::ClockDomainCrossingRequiresBridge { issue, .. } => {
                vec![(**issue).clone()]
            }
        }
    }

    pub(super) fn from_issues(issues: Vec<RuntimeIssue>) -> Self {
        Self::InvalidProject {
            issues: issues.into_boxed_slice(),
        }
    }

    pub(super) fn from_project_validation_report(report: crate::ProjectValidationReport) -> Self {
        Self::InvalidProject {
            issues: report
                .errors()
                .iter()
                .map(|error| {
                    RuntimeIssue::structured_error(
                        "audio-dsp.invalid-project",
                        error.message.clone(),
                        json!({ "surface": "internal-project-validation" }),
                    )
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        }
    }

    pub(super) fn from_plan_error(error: PlanError) -> Self {
        match error {
            PlanError::InvalidProject(report) => Self::from_project_validation_report(report),
            error => {
                let message = error.to_string();
                Self::Plan {
                    issues: vec![RuntimeIssue::structured_error(
                        "audio-dsp.plan",
                        message.clone(),
                        json!({ "surface": "internal-plan" }),
                    )]
                    .into_boxed_slice(),
                    message,
                }
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum AudioOfflineDspError {
    #[error("{0}")]
    Plan(#[from] AudioDspPlanError),
    #[error("audio offline dsp block count must be greater than zero")]
    InvalidBlockCount { issue: Box<RuntimeIssue> },
    #[error("offline audio dsp node {node_id} uses unsupported kind {kind}")]
    UnsupportedNodeKind {
        node_id: String,
        kind: String,
        issue: Box<RuntimeIssue>,
    },
}

impl AudioOfflineDspError {
    pub fn issues(&self) -> Vec<RuntimeIssue> {
        match self {
            Self::Plan(error) => error.issues(),
            Self::InvalidBlockCount { issue } | Self::UnsupportedNodeKind { issue, .. } => {
                vec![(**issue).clone()]
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum AudioRealtimeDspError {
    #[error("{0}")]
    Plan(#[from] AudioDspPlanError),
    #[error("audio realtime dsp output channel count must be greater than zero")]
    InvalidChannelCount { issue: Box<RuntimeIssue> },
    #[error(
        "audio realtime dsp graph must contain exactly one object.core.audio.output node, found {count}"
    )]
    OutputCount {
        count: usize,
        issue: Box<RuntimeIssue>,
    },
    #[error("audio realtime dsp node {node_id} uses unsupported kind {kind}")]
    UnsupportedNodeKind {
        node_id: String,
        kind: String,
        issue: Box<RuntimeIssue>,
    },
}

impl AudioRealtimeDspError {
    pub fn issues(&self) -> Vec<RuntimeIssue> {
        match self {
            Self::Plan(error) => error.issues(),
            Self::InvalidChannelCount { issue }
            | Self::OutputCount { issue, .. }
            | Self::UnsupportedNodeKind { issue, .. } => vec![(**issue).clone()],
        }
    }
}
