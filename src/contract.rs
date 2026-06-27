use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub use skenion_contracts::{
    AudioClockBridgeMethodV01 as AudioClockBridgeMethod,
    AudioClockBridgePlanV01 as AudioClockBridgePlan,
    AudioClockDomainAuthorityV01 as AudioClockDomainAuthority,
    AudioClockDomainV01 as AudioClockDomain, AudioDeviceDescriptorV01 as AudioDeviceDescriptor,
    AudioDevicePreferenceV01 as AudioDevicePreference,
    AudioEndpointDirectionV01 as AudioEndpointDirection, AudioEndpointV01 as AudioEndpoint,
    AudioGraphPartitionV01 as AudioGraphPartition, AudioResamplerPlanV01 as AudioResamplerPlan,
    AudioStreamConfigRequestV01 as AudioStreamConfigRequest,
    AudioStreamConfigResolvedV01 as AudioStreamConfigResolved, ClockAuthorityV01 as ClockAuthority,
    ClockCapabilityV01 as ClockCapability, ClockFieldV01 as ClockField,
    ClockSourceKindV01 as ClockSourceKind, ClockStateV01 as ClockState,
    ClockTimeSignatureV01 as ClockTimeSignature, DataFlowV01 as DataFlow, DataTypeV01 as DataType,
    EndpointBindingDeliveryPolicyV01 as EndpointBindingDeliveryPolicy,
    EndpointBindingValueFormatV01 as EndpointBindingValueFormat,
    ExecutionModelV01 as ExecutionModel,
    MIDI_CLOCK_TICKS_PER_QUARTER_V01 as MIDI_CLOCK_TICKS_PER_QUARTER,
    MIDI_CLOCK_TICKS_PER_SIXTEENTH_V01 as MIDI_CLOCK_TICKS_PER_SIXTEENTH,
    MidiClockApplyResultV01 as MidiClockApplyResult,
    MidiClockDiagnosticSeverityV01 as MidiClockDiagnosticSeverity,
    MidiClockDiagnosticV01 as MidiClockDiagnostic, MidiClockMessageKindV01 as MidiClockMessageKind,
    MidiClockMessageV01 as MidiClockMessage, MidiClockSnapshotV01 as MidiClockSnapshot,
    NodeExecutionV01 as NodeExecution, NodeStateV01 as NodeState, NodeSurfaceV01 as NodeSurface,
    NumberRangeV01 as NumberRange, PortActivationV01 as PortActivation,
    PortDirectionV01 as PortDirection, PortV01 as Port,
    ShaderInterfaceDiagnosticV01 as ShaderInterfaceDiagnostic,
    ShaderInterfaceV01 as ShaderInterface, ShaderUniformV01 as ShaderUniform,
    StringOrStringsV01 as StringOrStrings, ValueEndpointRefV01 as ValueEndpointRef,
    ValueFormatV01 as ValueFormat, ValueOccurrenceHeaderV01 as ValueOccurrenceHeader,
    ValuePayloadKindV01 as ValuePayloadKind, analyze_shader_interface_v01,
    apply_midi_clock_message_v01 as apply_midi_clock_message,
    midi_clock_snapshot_to_clock_state_v01 as midi_clock_snapshot_to_clock_state,
    parse_midi_clock_message_v01 as parse_midi_clock_message,
    plan_audio_clock_bridge_v01 as plan_audio_clock_bridge, shader_interface_to_ports_v01,
};
pub use skenion_contracts::{
    CanvasNodeViewV01 as CanvasNodeView, CanvasViewStateV01 as CanvasViewState,
    CanvasViewportV01 as CanvasViewport, CycleValidationV01 as CycleValidationCurrent,
    EdgeEndpointV01 as EdgeEndpointCurrent, EdgeSpecV01 as EdgeSpecCurrent,
    ExecutionModelV01 as ExecutionModelCurrent, ExtensionKindV01 as ExtensionKind,
    ExtensionManifestV01 as ExtensionManifest,
    ExtensionNativeArtifactV01 as ExtensionNativeArtifact,
    ExtensionNativeBindingV01 as ExtensionNativeBinding, ExtensionProvidesV01 as ExtensionProvides,
    FanOutPolicyV01 as FanOutPolicyCurrent, FeedbackBoundaryV01 as FeedbackBoundaryCurrent,
    FeedbackPolicyV01 as FeedbackPolicyCurrent, GraphDocumentV01 as GraphDocumentCurrent,
    GraphFragmentOutsideEndpointPolicyV01 as GraphFragmentOutsideEndpointPolicyCurrent,
    GraphFragmentV01 as GraphFragmentCurrent, GraphNodeV01 as GraphNodeCurrent, GraphTargetRef,
    GraphValidationResultV01 as GraphValidationResultCurrent, IdConflictPolicy,
    MergePolicyV01 as MergePolicyCurrent, NodeDefinitionManifestV01 as NodeDefinitionCurrent,
    PasteGraphFragmentRequest, PastePlacement, PatchContractPortV01 as PatchContractPortCurrent,
    PatchContractV01 as PatchContractCurrent, PatchDefinitionV01 as PatchDefinitionCurrent,
    PatchPath, PortDirectionV01 as PortDirectionCurrent, PortRateV01 as PortRateCurrent,
    PortSpecV01 as PortSpecCurrent, ProjectDocumentV01 as ProjectDocumentCurrent,
    ProjectMetadataV01 as ProjectMetadataCurrent, ViewStateV01 as ViewState,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortRef {
    pub node: String,
    pub port: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: PortRef,
    pub to: PortRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub kind: String,
    pub kind_version: String,
    pub params: serde_json::Map<String, Value>,
    pub ports: Vec<Port>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct GraphDocument {
    pub schema: String,
    pub schema_version: String,
    pub id: String,
    pub revision: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct NodeDefinition {
    pub schema: String,
    pub schema_version: String,
    pub id: String,
    pub version: String,
    pub display_name: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_api_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<NodeSurface>,
    pub ports: Vec<Port>,
    pub execution: NodeExecution,
    pub state: NodeState,
    pub permissions: Vec<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct GraphPatch {
    pub schema: String,
    pub schema_version: String,
    pub id: String,
    pub base_revision: String,
    pub ops: Vec<GraphPatchOperation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
#[serde(rename_all_fields = "camelCase")]
pub enum GraphPatchOperation {
    #[serde(rename = "addNode")]
    AddNode { node: GraphNode },
    #[serde(rename = "removeNode")]
    RemoveNode { node_id: String },
    #[serde(rename = "replaceNode")]
    ReplaceNode { node_id: String, node: GraphNode },
    #[serde(rename = "setNodeParam")]
    SetNodeParam {
        node_id: String,
        key: String,
        value: Value,
    },
    #[serde(rename = "addEdge")]
    AddEdge { edge: Edge },
    #[serde(rename = "removeEdge")]
    RemoveEdge { edge: Edge },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct ApplyPatchError {
    pub message: String,
}

impl ApplyPatchError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct InvertPatchError {
    pub message: String,
}

impl InvertPatchError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
