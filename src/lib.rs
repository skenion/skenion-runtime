mod contract;
mod loader;
mod planner;
mod preview_manager;
mod project;
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
    ApplyPatchError, DataFlow, DataType, Edge, ExecutionModel, GraphDocument, GraphNode,
    GraphPatch, GraphPatchEvent, GraphPatchEventKind, GraphPatchHistory, GraphPatchOperation,
    InvertPatchError, NodeDefinition, NodeExecution, NodeState, NumberRange, Port, PortActivation,
    PortDirection, PortRef, StringOrStrings,
};
pub use loader::{LoadError, load_graph_document, load_node_definition};
pub use planner::{
    ExecutionGroup, ExecutionPlan, PlanEdge, PlanError, PlanNode, build_execution_plan,
    format_plan_text,
};
pub use preview_manager::{
    PreviewContext, PreviewManager, PreviewState, RuntimePreviewStartRequest,
    RuntimePreviewStatusResponse,
};
pub use project::{ProjectValidationError, ProjectValidationReport, validate_project};
pub use registry::{NodeDefinitionKey, NodeRegistry, RegistryError, RegistryLoadError};
pub use render::{
    ClearColorScene, DEFAULT_CLEAR_COLOR, FullscreenShaderScene, PREVIEW_DOCUMENT_SCHEMA,
    PREVIEW_DOCUMENT_SCHEMA_VERSION, PreviewDocument, RENDER_CLEAR_COLOR_KIND,
    RENDER_FULLSCREEN_SHADER_KIND, RenderScene, RenderSceneBuildError, ShaderLanguage,
    render_scene_from_preview_document, run_render_preview_window, write_preview_document,
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
    RuntimePatchResponse, RuntimeSession, RuntimeSessionResponse, RuntimeSessionSnapshot,
    SessionRunRequest,
};
pub use telemetry::{
    PREVIEW_TELEMETRY_SCHEMA, PREVIEW_TELEMETRY_SCHEMA_VERSION, PreviewTelemetryHeartbeat,
    PreviewTelemetryWriter, RuntimeTelemetryPreview, RuntimeTelemetryProcess,
    RuntimeTelemetryRender, RuntimeTelemetrySession, RuntimeTelemetrySnapshot, TELEMETRY_SCHEMA,
    TELEMETRY_SCHEMA_VERSION, preview_telemetry_path, read_preview_telemetry, unix_ms_timestamp,
    write_preview_telemetry_heartbeat,
};
pub use validation::{
    ValidationError, ValidationReport, apply_graph_patch, compatible_data_types,
    invert_graph_patch, type_label, validate_graph_document, validate_node_definition,
};
pub use visual::{PreviewFrameLimit, run_preview_window};
