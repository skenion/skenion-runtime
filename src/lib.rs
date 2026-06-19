mod contract;
mod control_state;
mod control_value;
mod conversion;
mod loader;
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

pub use contract::{
    ApplyPatchError, CycleValidationV02, DataFlow, DataType, Edge, EdgeSpecV02, ExecutionModel,
    ExecutionModelV02, FanOutPolicyV02, FeedbackBoundaryV02, FeedbackPolicyV02, GraphDocument,
    GraphDocumentV02, GraphNode, GraphNodeV02, GraphPatch, GraphPatchEvent, GraphPatchEventKind,
    GraphPatchHistory, GraphPatchOperation, GraphValidationResultV02, InvertPatchError,
    MergePolicyV02, NodeDefinition, NodeDefinitionV02, NodeExecution, NodeState, NumberRange, Port,
    PortActivation, PortDirection, PortDirectionV02, PortRef, PortSpecV02,
    ReplaceNodeInterfaceEdgePolicy, ShaderInterface, ShaderInterfaceDiagnostic, ShaderUniform,
    StringOrStrings, analyze_shader_interface_v01, shader_interface_to_ports_v01,
};
pub use control_state::{
    ControlState, RuntimeControlEmission, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeControlReadRequest, RuntimeControlReadResponse, RuntimeControlReadTarget,
    RuntimeControlStateResponse, read_graph_param, read_graph_port,
};
pub use control_value::{ControlMessage, ControlValue};
pub use conversion::{convert_control_value_to_data_kind, convert_control_value_to_stored};
pub use loader::{LoadError, load_graph_document, load_node_definition};
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
