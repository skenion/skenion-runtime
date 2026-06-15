mod contract;
mod loader;
mod planner;
mod project;
mod registry;
mod validation;

pub use contract::{
    DataFlow, DataType, Edge, ExecutionModel, GraphDocument, GraphNode, NodeDefinition,
    NodeExecution, NodeState, NumberRange, Port, PortActivation, PortDirection, PortRef,
    StringOrStrings,
};
pub use loader::{LoadError, load_graph_document, load_node_definition};
pub use planner::{
    ExecutionGroup, ExecutionPlan, PlanEdge, PlanError, PlanNode, build_execution_plan,
    format_plan_text,
};
pub use project::{ProjectValidationError, ProjectValidationReport, validate_project};
pub use registry::{NodeDefinitionKey, NodeRegistry, RegistryError, RegistryLoadError};
pub use validation::{
    ValidationError, ValidationReport, compatible_data_types, type_label, validate_graph_document,
    validate_node_definition,
};
