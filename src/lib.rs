mod audio_backend;
mod clock;
mod collaboration;
mod contract;
mod control_state;
mod control_value;
mod conversion;
mod dsp;
mod extension_manager;
mod io_device_manager;
mod log_store;
#[cfg(not(test))]
mod midi_input;
mod planner;
mod preview_control_state;
mod preview_manager;
#[allow(dead_code)]
mod project;
mod project_current;
#[allow(dead_code)]
mod registry;
mod render;
mod runtime_time;
mod scheduler;
mod serve;
mod server;
mod session;
mod session_registry;
mod sidecar;
mod telemetry;
#[allow(dead_code, unused_imports)]
mod validation;
mod visual;

pub use audio_backend::{
    AudioBackendConfig, AudioBackendError, AudioBackendInfo, RunningAudioBackend,
    start_default_audio_output_backend,
};
pub use clock::{
    MidiClockAdapter, MidiClockFixtureError, MidiSongPositionSource,
    RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA, RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION,
    RuntimeClockDiagnostic, RuntimeClockDiagnosticSeverity, RuntimeMidiClockFixture,
    RuntimeMidiClockFixtureEvent, RuntimeMidiClockFixtureReport, RuntimeMidiClockSourceId,
    RuntimeMidiClockSourceKind, RuntimeMidiClockStateSnapshot, RuntimeMidiClockTimeline,
    TimestampedMidiMessage, format_midi_clock_fixture_report_text, run_midi_clock_fixture,
    run_midi_clock_fixture_file,
};
pub use collaboration::{
    COLLABORATION_EVENT_REPLAY_LIMIT, RuntimeCollaborationLog, RuntimeCollaborationReplay,
    collaboration_broadcast_event_after_high_water, collaboration_event,
};
#[allow(unused_imports)]
pub(crate) use contract::{
    ApplyPatchError, AudioClockBridgeMethod, AudioClockBridgePlan, AudioClockDomain,
    AudioClockDomainAuthority, AudioDeviceDescriptor, AudioDevicePreference, AudioEndpoint,
    AudioEndpointDirection, AudioGraphPartition, AudioResamplerPlan, AudioStreamConfigRequest,
    AudioStreamConfigResolved, ClockAuthority, ClockCapability, ClockField, ClockSourceKind,
    ClockState, ClockTimeSignature, DataFlow, DataType, Edge, ExecutionModel, ExtensionKind,
    ExtensionManifest, ExtensionNativeArtifact, ExtensionNativeBinding, ExtensionProvides,
    GraphDocument, GraphNode, GraphPatch, GraphPatchOperation, InvertPatchError,
    MIDI_CLOCK_TICKS_PER_QUARTER, MIDI_CLOCK_TICKS_PER_SIXTEENTH, MidiClockApplyResult,
    MidiClockDiagnostic, MidiClockDiagnosticSeverity, MidiClockMessage, MidiClockMessageKind,
    MidiClockSnapshot, NodeDefinition, NodeExecution, NodeState, NumberRange, Port, PortActivation,
    PortDirection, PortRef, ShaderInterface, ShaderInterfaceDiagnostic, ShaderUniform,
    StringOrStrings, analyze_shader_interface_v01, apply_midi_clock_message,
    midi_clock_snapshot_to_clock_state, parse_midi_clock_message, plan_audio_clock_bridge,
    shader_interface_to_ports_v01,
};
pub use contract::{
    CanvasNodeView, CanvasViewState, CanvasViewport, CycleValidationCurrent, EdgeEndpointCurrent,
    EdgeSpecCurrent, ExecutionModelCurrent, FanOutPolicyCurrent, FeedbackBoundaryCurrent,
    FeedbackPolicyCurrent, GraphDocumentCurrent, GraphFragmentCurrent,
    GraphFragmentOutsideEndpointPolicyCurrent, GraphNodeCurrent, GraphTargetRef,
    GraphValidationResultCurrent, IdConflictPolicy, IdRemapResult, MergePolicyCurrent,
    NodeDefinitionCurrent, PasteGraphFragmentRequest, PasteGraphFragmentResponse, PastePlacement,
    PatchContractCurrent, PatchContractPortCurrent, PatchDefinitionCurrent, PatchPath,
    PortDirectionCurrent, PortRateCurrent, PortSpecCurrent, ProjectDocumentCurrent,
    ProjectMetadataCurrent, RuntimeCollaborationAck, RuntimeCollaborationAuthSubject,
    RuntimeCollaborationCausalMetadata, RuntimeCollaborationChange, RuntimeCollaborationConflict,
    RuntimeCollaborationEventEnvelope, RuntimeCollaborationEventKind,
    RuntimeCollaborationEventPayload, RuntimeCollaborationNack, RuntimeCollaborationNackReason,
    RuntimeCollaborationOperationBatch, RuntimeCollaborationOperationBatchResult,
    RuntimeCollaborationOperationDiagnostic, RuntimeCollaborationOperationEnvelope,
    RuntimeCollaborationOperationPayload, RuntimeCollaborationOperationResult,
    RuntimeCollaborationOperationStatus, RuntimeCollaborationPresenceEnvelope,
    RuntimeCollaborationRebase, RuntimeCollaborationRebaseStrategy,
    RuntimeCollaborationSelectionEnvelope, RuntimeCollaborationServerClock,
    RuntimeCollaborationUndoRedoAction, RuntimeCollaborationUndoScope,
    RuntimeCollaborationUndoScopeKind, RuntimeOperationAttribution, RuntimeOperationDiagnostic,
    RuntimeOperationEnvelope, ViewState, validate_runtime_collaboration_event_envelope,
    validate_runtime_collaboration_operation_batch,
    validate_runtime_collaboration_operation_batch_result,
    validate_runtime_collaboration_operation_envelope,
    validate_runtime_collaboration_operation_result,
    validate_runtime_collaboration_presence_envelope,
    validate_runtime_collaboration_selection_envelope,
};
pub use control_state::{
    ControlState, RuntimeControlEmission, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeControlReadRequest, RuntimeControlReadResponse, RuntimeControlReadTarget,
    RuntimeControlStateResponse,
};
pub(crate) use control_state::{read_graph_param, read_graph_port};
pub use control_value::{ControlMessage, ControlValue};
pub use conversion::{convert_control_value_to_data_kind, convert_control_value_to_stored};
pub use dsp::{
    AudioDspBlockReport, AudioDspBuffer, AudioDspControlInput, AudioDspPlan, AudioDspPlanEdge,
    AudioDspPlanError, AudioDspPlanNode, AudioDspPlanOptions, AudioDspRenderedBuffer,
    AudioDspSignalInput, AudioDspSignalOutput, AudioDspSnapshot, AudioEndpointPlanNode,
    AudioOfflineDspError, AudioOfflineDspOptions, AudioOfflineDspReport, AudioRealtimeDspError,
    AudioRealtimeDspExecutor, AudioRealtimeDspOptions, build_audio_dsp_plan_current,
    run_offline_audio_dsp_current,
};
pub use extension_manager::{
    RUNTIME_EXTENSION_ABI_VERSION, RUNTIME_EXTENSION_MANIFEST_FILE, RuntimeExtensionDescriptor,
    RuntimeExtensionListResponse, RuntimeExtensionManager, RuntimeExtensionRegistrySnapshot,
    RuntimeExtensionStatus, SKENION_EXTENSION_PATH_ENV,
};
pub use io_device_manager::{
    RuntimeIoBindingConfig, RuntimeIoDeviceDescriptor, RuntimeIoDeviceListResponse,
    RuntimeIoDeviceManager, RuntimeIoDiagnostic, RuntimeIoDiagnosticSeverity, RuntimeIoDirection,
    RuntimeIoInlineFrame, RuntimeIoTransportKind,
};
pub use log_store::{
    DEFAULT_RUNTIME_LOG_BACKLOG_LIMIT, RUNTIME_LOG_SCHEMA, RUNTIME_LOG_SCHEMA_VERSION,
    RuntimeLogEvent, RuntimeLogRetention, RuntimeLogSnapshotResponse, RuntimeLogSource,
    RuntimeLogStore,
};
pub(crate) use planner::build_execution_plan;
pub(crate) use planner::{ExecutionGroup, PlanEdge, PlanEdgeMetadata, PlanError, PlanNode};
pub use planner::{ExecutionPlan, format_plan_text};
pub use preview_control_state::{
    PREVIEW_CONTROL_STATE_SCHEMA, PREVIEW_CONTROL_STATE_SCHEMA_VERSION,
    PreviewControlStateSnapshot, preview_control_state_path, read_preview_control_state_snapshot,
    write_preview_control_state_snapshot,
};
pub(crate) use preview_manager::PreviewContext;
pub use preview_manager::{
    PreviewManager, PreviewState, RuntimePreviewStartRequest, RuntimePreviewStatusResponse,
};
pub(crate) use project::{ProjectValidationReport, validate_project};
pub use project_current::{
    CURRENT_SCHEMA_VERSION, ProjectRequestCurrent, RunProjectRequestCurrent,
    build_execution_plan_current, build_execution_plan_request_current,
    build_execution_plan_run_request_current, expand_project_graph_current,
    project_document_payload_schema_diagnostics, project_document_validation_diagnostics_current,
    schema_version_diagnostic, validate_project_current, validate_project_request_current,
};
pub(crate) use registry::NodeRegistry;
pub use render::{
    ClearColorScene, DEFAULT_CLEAR_COLOR, FullscreenShaderScene, GeneratedShaderResponse,
    GeneratedShaderSource, GeneratedShaderSourceMap, RENDER_CLEAR_COLOR_KIND,
    RENDER_FULLSCREEN_SHADER_KIND, RENDER_OUTPUT_KIND, RenderScene, RenderSceneBuildError,
    ShaderLanguage, ShaderUniformBinding, ShaderUniformValue, run_render_preview_document_file,
};
pub(crate) use render::{PreviewDocument, generated_shader_response_from_preview_document};
pub use scheduler::{
    DummyExecutionReport, DummyFrameReport, DummyNodeExecution, format_dummy_execution_text,
    run_dummy_execution,
};
pub use serve::{ServeRuntimeOptions, serve_runtime, serve_runtime_with_options};
pub use server::{
    DEFAULT_HOST, DEFAULT_PORT, DiagnosticSeverity, HealthResponse, RUNTIME_API_VERSION,
    RuntimeApiResponse, RuntimeDiagnostic, RuntimeInfoResponse, RuntimeServerState, runtime_router,
    runtime_router_with_state,
};
pub use session::{
    RuntimeHistory, RuntimeHistoryEntry, RuntimeHistoryEntryKind, RuntimeMutationRequest,
    RuntimePatchResponse, RuntimeSession, RuntimeSessionResponse, RuntimeSessionSnapshot,
    RuntimeViewPatch, RuntimeViewPatchOperation, SessionRunRequest,
};
pub use session_registry::{
    DEFAULT_SESSION_ID, RuntimeSessionEvent, RuntimeSessionEventKind, RuntimeSessionRecord,
    RuntimeSessionRegistry, SessionEventsQuery,
};
pub use sidecar::{
    RuntimeEndpointConfig, RuntimeSidecarHealthResponse, RuntimeSidecarShutdownInfo,
    RuntimeSidecarShutdownRequest, RuntimeSidecarShutdownResponse, RuntimeSidecarStartupResponse,
    RuntimeSidecarTokenInfo,
};
pub use telemetry::{
    PREVIEW_TELEMETRY_SCHEMA, PREVIEW_TELEMETRY_SCHEMA_VERSION, PreviewTelemetryHeartbeat,
    PreviewTelemetryWriter, RuntimeTelemetryPreview, RuntimeTelemetryProcess,
    RuntimeTelemetryRender, RuntimeTelemetrySession, RuntimeTelemetrySnapshot, ShaderDiagnostic,
    ShaderDiagnosticPhase, ShaderDiagnosticSeverity, ShaderDiagnosticSource, TELEMETRY_SCHEMA,
    TELEMETRY_SCHEMA_VERSION, preview_telemetry_path, read_preview_telemetry, unix_ms_timestamp,
    write_preview_telemetry_heartbeat,
};
pub(crate) use validation::{
    ValidationReport, compatible_data_types, type_label, validate_graph_document,
    validate_node_definition,
};
pub use visual::{PreviewFrameLimit, run_preview_window};
