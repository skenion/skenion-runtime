mod contract;
mod loader;
mod planner;
mod project;
mod registry;
mod scheduler;
mod serve;
mod server;
mod session;
mod validation;
mod visual;

pub use contract::{
    ApplyPatchError, DataFlow, DataType, Edge, ExecutionModel, GraphDocument, GraphNode,
    GraphPatch, GraphPatchOperation, NodeDefinition, NodeExecution, NodeState, NumberRange, Port,
    PortActivation, PortDirection, PortRef, StringOrStrings,
};
pub use loader::{LoadError, load_graph_document, load_node_definition};
pub use planner::{
    ExecutionGroup, ExecutionPlan, PlanEdge, PlanError, PlanNode, build_execution_plan,
    format_plan_text,
};
pub use project::{ProjectValidationError, ProjectValidationReport, validate_project};
pub use registry::{NodeDefinitionKey, NodeRegistry, RegistryError, RegistryLoadError};
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
    ValidationError, ValidationReport, apply_graph_patch, compatible_data_types, type_label,
    validate_graph_document, validate_node_definition,
};
pub use visual::run_preview_window;
