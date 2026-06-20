mod audio_backend;
mod clock;
mod contract;
mod control_state;
mod control_value;
mod conversion;
mod dsp;
mod loader;
mod midi_input;
mod planner;
mod preview_control_state;
mod preview_manager;
mod project;
mod project_v02;
mod registry;
mod render;
mod scheduler;
mod serve;
mod server;
mod session;
mod telemetry;
mod validation;
mod visual;

pub use audio_backend::{
    AudioBackendConfig, AudioBackendError, AudioBackendInfo, RunningAudioBackend,
    start_default_audio_output_backend,
};
pub use clock::{
    ClockSourceStore, MidiClockAdapter, MidiClockFixtureError, MidiSongPositionSource,
    RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA, RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION,
    RuntimeClockDiagnostic, RuntimeClockDiagnosticSeverity, RuntimeClockSnapshot,
    RuntimeClockSourceId, RuntimeClockSourceKind, RuntimeMidiClockFixture,
    RuntimeMidiClockFixtureEvent, RuntimeMidiClockFixtureReport, TimestampedMidiMessage,
    format_midi_clock_fixture_report_text, run_midi_clock_fixture, run_midi_clock_fixture_file,
};
pub use contract::{
    ApplyPatchError, AudioClockBridgeMethod, AudioClockBridgePlan, AudioClockDomain,
    AudioClockDomainAuthority, AudioDeviceDescriptor, AudioDevicePreference, AudioEndpoint,
    AudioEndpointDirection, AudioGraphPartition, AudioResamplerPlan, AudioStreamConfigRequest,
    AudioStreamConfigResolved, ClockAuthority, ClockCapability, ClockField, ClockSourceKind,
    ClockState, ClockTimeSignature, CycleValidationV02, DataFlow, DataType, Edge, EdgeSpecV02,
    ExecutionModel, ExecutionModelV02, FanOutPolicyV02, FeedbackBoundaryV02, FeedbackPolicyV02,
    GraphDocument, GraphDocumentV02, GraphNode, GraphNodeV02, GraphPatch, GraphPatchEvent,
    GraphPatchEventKind, GraphPatchHistory, GraphPatchOperation, GraphValidationResultV02,
    InvertPatchError, MIDI_CLOCK_TICKS_PER_QUARTER, MIDI_CLOCK_TICKS_PER_SIXTEENTH, MergePolicyV02,
    MidiClockApplyResult, MidiClockDiagnostic, MidiClockDiagnosticSeverity, MidiClockMessage,
    MidiClockMessageKind, MidiClockSnapshot, NodeDefinition, NodeDefinitionV02, NodeExecution,
    NodeState, NumberRange, Port, PortActivation, PortDirection, PortDirectionV02, PortRef,
    PortSpecV02, ReplaceNodeInterfaceEdgePolicy, ShaderInterface, ShaderInterfaceDiagnostic,
    ShaderUniform, StringOrStrings, analyze_shader_interface_v01, apply_midi_clock_message,
    midi_clock_snapshot_to_clock_state, parse_midi_clock_message, plan_audio_clock_bridge,
    shader_interface_to_ports_v01,
};
pub use control_state::{
    ControlState, RuntimeControlEmission, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeControlReadRequest, RuntimeControlReadResponse, RuntimeControlReadTarget,
    RuntimeControlStateResponse, read_graph_param, read_graph_port,
};
pub use control_value::{ControlMessage, ControlValue};
pub use conversion::{convert_control_value_to_data_kind, convert_control_value_to_stored};
pub use dsp::{
    AudioDspBlockReport, AudioDspBuffer, AudioDspControlInput, AudioDspPlan, AudioDspPlanEdge,
    AudioDspPlanError, AudioDspPlanNode, AudioDspPlanOptions, AudioDspRenderedBuffer,
    AudioDspSignalInput, AudioDspSignalOutput, AudioDspSnapshot, AudioEndpointPlanNode,
    AudioOfflineDspError, AudioOfflineDspOptions, AudioOfflineDspReport, AudioRealtimeDspError,
    AudioRealtimeDspExecutor, AudioRealtimeDspOptions, build_audio_dsp_plan, run_offline_audio_dsp,
};
pub use loader::{LoadError, load_graph_document, load_node_definition};
pub use midi_input::{
    RUNTIME_MIDI_CLOCK_INPUT_SCHEMA, RUNTIME_MIDI_CLOCK_INPUT_SCHEMA_VERSION,
    RUNTIME_MIDI_INPUT_SCHEMA, RUNTIME_MIDI_INPUT_SCHEMA_VERSION, RuntimeMidiClockInputReport,
    RuntimeMidiClockInputRequest, RuntimeMidiInputListReport, RuntimeMidiInputPort,
    format_midi_clock_input_report_text, format_midi_input_list_report_text, list_midi_input_ports,
    run_midi_clock_input,
};
pub use planner::{
    ExecutionGroup, ExecutionPlan, PlanEdge, PlanEdgeMetadata, PlanError, PlanNode,
    build_execution_plan, format_plan_text,
};
pub use preview_control_state::{
    PREVIEW_CONTROL_STATE_SCHEMA, PREVIEW_CONTROL_STATE_SCHEMA_VERSION,
    PreviewControlStateSnapshot, preview_control_state_path, read_preview_control_state_snapshot,
    write_preview_control_state_snapshot,
};
pub use preview_manager::{
    PreviewContext, PreviewManager, PreviewState, RuntimePreviewStartRequest,
    RuntimePreviewStatusResponse,
};
pub use project::{ProjectValidationError, ProjectValidationReport, validate_project};
pub use project_v02::{
    ProjectRequestV02, RunProjectRequestV02, build_execution_plan_v02, validate_project_v02,
};
pub use registry::{NodeDefinitionKey, NodeRegistry, RegistryError, RegistryLoadError};
pub use render::{
    ClearColorScene, DEFAULT_CLEAR_COLOR, FullscreenShaderScene, GeneratedShaderResponse,
    GeneratedShaderSource, GeneratedShaderSourceMap, PREVIEW_DOCUMENT_SCHEMA,
    PREVIEW_DOCUMENT_SCHEMA_VERSION, PreviewDocument, RENDER_CLEAR_COLOR_KIND,
    RENDER_FULLSCREEN_SHADER_KIND, RENDER_OUTPUT_KIND, RenderScene, RenderSceneBuildError,
    ShaderLanguage, ShaderUniformBinding, ShaderUniformValue,
    generated_shader_response_from_preview_document, render_scene_from_preview_document,
    run_render_preview_window, write_preview_document,
};
pub use scheduler::{
    DummyExecutionReport, DummyFrameReport, DummyNodeExecution, format_dummy_execution_text,
    run_dummy_execution,
};
pub use serve::serve_runtime;
pub use server::{
    DEFAULT_HOST, DEFAULT_PORT, DiagnosticSeverity, HealthResponse, ProjectRequest,
    RUNTIME_API_VERSION, RunProjectRequest, RuntimeApiResponse, RuntimeDiagnostic,
    RuntimeInfoResponse, RuntimeServerState, runtime_router, runtime_router_with_state,
};
pub use session::{
    RuntimePatchResponse, RuntimeSession, RuntimeSessionProjectResponse, RuntimeSessionResponse,
    RuntimeSessionSnapshot, SessionRunRequest,
};
pub use telemetry::{
    PREVIEW_TELEMETRY_SCHEMA, PREVIEW_TELEMETRY_SCHEMA_VERSION, PreviewTelemetryHeartbeat,
    PreviewTelemetryWriter, RuntimeTelemetryPreview, RuntimeTelemetryProcess,
    RuntimeTelemetryRender, RuntimeTelemetrySession, RuntimeTelemetrySnapshot, ShaderDiagnostic,
    ShaderDiagnosticPhase, ShaderDiagnosticSeverity, ShaderDiagnosticSource, TELEMETRY_SCHEMA,
    TELEMETRY_SCHEMA_VERSION, preview_telemetry_path, read_preview_telemetry, unix_ms_timestamp,
    write_preview_telemetry_heartbeat,
};
pub use validation::{
    ValidationError, ValidationReport, apply_graph_patch, compatible_data_types,
    invert_graph_patch, type_label, validate_graph_document, validate_node_definition,
};
pub use visual::{PreviewFrameLimit, run_preview_window};
