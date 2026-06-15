pub use skenion_contracts::{
    ValidationErrorV01 as ValidationError, ValidationReportV01 as ValidationReport,
};

use crate::{DataType, GraphDocument, NodeDefinition};

pub fn validate_node_definition(definition: &NodeDefinition) -> Result<(), ValidationReport> {
    skenion_contracts::validate_node_definition_v01(definition)
}

pub fn validate_graph_document(graph: &GraphDocument) -> Result<(), ValidationReport> {
    skenion_contracts::validate_graph_document_v01(graph)
}

pub fn compatible_data_types(source_type: &DataType, target_type: &DataType) -> bool {
    skenion_contracts::compatible_data_types_v01(source_type, target_type)
}

pub fn type_label(data_type: &DataType) -> String {
    skenion_contracts::type_label_v01(data_type)
}
