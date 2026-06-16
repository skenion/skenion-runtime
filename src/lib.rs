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
    DEFAULT_CLEAR_COLOR, PREVIEW_DOCUMENT_SCHEMA, PREVIEW_DOCUMENT_SCHEMA_VERSION, PreviewDocument,
    RENDER_CLEAR_COLOR_KIND, RenderScene, render_scene_from_preview_document,
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
    RuntimePatchResponse, RuntimeSession, RuntimeSessionResponse, RuntimeSessionSnapshot,
    SessionRunRequest,
};
pub use validation::{
    ValidationError, ValidationReport, apply_graph_patch, compatible_data_types,
    invert_graph_patch, type_label, validate_graph_document, validate_node_definition,
};
pub use visual::{PreviewFrameLimit, run_preview_window};
