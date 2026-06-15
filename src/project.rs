use std::{collections::HashMap, error::Error, fmt};

use crate::{
    GraphDocument, GraphNode, NodeDefinition, NodeRegistry, Port, PortDirection,
    compatible_data_types, type_label, validate_graph_document,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectValidationError {
    pub message: String,
}

impl ProjectValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectValidationReport {
    errors: Vec<ProjectValidationError>,
}

impl ProjectValidationReport {
    fn new(errors: Vec<ProjectValidationError>) -> Self {
        Self { errors }
    }

    pub fn errors(&self) -> &[ProjectValidationError] {
        &self.errors
    }
}

impl fmt::Display for ProjectValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let messages = self
            .errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        write!(f, "{messages}")
    }
}

impl Error for ProjectValidationReport {}

pub fn validate_project(
    graph: &GraphDocument,
    registry: &NodeRegistry,
) -> Result<(), ProjectValidationReport> {
    let mut errors = Vec::new();

    if let Err(report) = validate_graph_document(graph) {
        errors.extend(
            report
                .errors()
                .iter()
                .map(|error| ProjectValidationError::new(error.message.clone())),
        );
    }

    let mut definitions_by_node: HashMap<&str, &NodeDefinition> = HashMap::new();
    for node in &graph.nodes {
        match registry.get(&node.kind, &node.kind_version) {
            Some(definition) => {
                validate_node_snapshot(node, definition, &mut errors);
                definitions_by_node.insert(node.id.as_str(), definition);
            }
            None => errors.push(ProjectValidationError::new(format!(
                "missing node definition: {}@{}",
                node.kind, node.kind_version
            ))),
        }
    }

    validate_edges(graph, &definitions_by_node, &mut errors);

    if detect_cycle(graph).is_some() {
        errors.push(ProjectValidationError::new(format!(
            "cycle detected: {}",
            cycle_node_list(graph)
        )));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ProjectValidationReport::new(errors))
    }
}

fn validate_node_snapshot(
    node: &GraphNode,
    definition: &NodeDefinition,
    errors: &mut Vec<ProjectValidationError>,
) {
    let definition_ports: HashMap<&str, &Port> = definition
        .ports
        .iter()
        .map(|port| (port.id.as_str(), port))
        .collect();
    let snapshot_ports: HashMap<&str, &Port> = node
        .ports
        .iter()
        .map(|port| (port.id.as_str(), port))
        .collect();

    for definition_port in &definition.ports {
        if !snapshot_ports.contains_key(definition_port.id.as_str()) {
            errors.push(ProjectValidationError::new(format!(
                "port snapshot missing manifest port: {}.{}",
                node.id, definition_port.id
            )));
        }
    }

    for snapshot_port in &node.ports {
        let Some(definition_port) = definition_ports.get(snapshot_port.id.as_str()) else {
            errors.push(ProjectValidationError::new(format!(
                "port snapshot references missing manifest port: {}.{}",
                node.id, snapshot_port.id
            )));
            continue;
        };

        if snapshot_port.direction != definition_port.direction {
            errors.push(ProjectValidationError::new(format!(
                "port snapshot mismatch: {}.{} direction {:?} != definition direction {:?}",
                node.id, snapshot_port.id, snapshot_port.direction, definition_port.direction
            )));
        }

        if snapshot_port.data_type.flow != definition_port.data_type.flow {
            errors.push(ProjectValidationError::new(format!(
                "port snapshot mismatch: {}.{} flow {:?} != definition flow {:?}",
                node.id,
                snapshot_port.id,
                snapshot_port.data_type.flow,
                definition_port.data_type.flow
            )));
        }

        if snapshot_port.data_type.data_kind != definition_port.data_type.data_kind {
            errors.push(ProjectValidationError::new(format!(
                "port snapshot mismatch: {}.{} dataKind {} != definition dataKind {}",
                node.id,
                snapshot_port.id,
                snapshot_port.data_type.data_kind,
                definition_port.data_type.data_kind
            )));
        }

        if !compatible_data_types(&snapshot_port.data_type, &definition_port.data_type) {
            errors.push(ProjectValidationError::new(format!(
                "port snapshot mismatch: {}.{} type {} is not compatible with definition type {}",
                node.id,
                snapshot_port.id,
                type_label(&snapshot_port.data_type),
                type_label(&definition_port.data_type)
            )));
        }
    }
}

fn validate_edges(
    graph: &GraphDocument,
    definitions_by_node: &HashMap<&str, &NodeDefinition>,
    errors: &mut Vec<ProjectValidationError>,
) {
    for edge in &graph.edges {
        let from_definition = definitions_by_node.get(edge.from.node.as_str());
        let to_definition = definitions_by_node.get(edge.to.node.as_str());
        let from = from_definition.and_then(|definition| {
            definition
                .ports
                .iter()
                .find(|port| port.id == edge.from.port)
        });
        let to = to_definition
            .and_then(|definition| definition.ports.iter().find(|port| port.id == edge.to.port));

        if from_definition.is_some() && from.is_none() {
            errors.push(ProjectValidationError::new(format!(
                "edge references missing manifest source port {}:{}",
                edge.from.node, edge.from.port
            )));
        }
        if to_definition.is_some() && to.is_none() {
            errors.push(ProjectValidationError::new(format!(
                "edge references missing manifest target port {}:{}",
                edge.to.node, edge.to.port
            )));
        }

        let (Some(from), Some(to)) = (from, to) else {
            continue;
        };

        if from.direction != PortDirection::Output {
            errors.push(ProjectValidationError::new(format!(
                "edge source {}:{} is not an output port",
                edge.from.node, edge.from.port
            )));
        }
        if to.direction != PortDirection::Input {
            errors.push(ProjectValidationError::new(format!(
                "edge target {}:{} is not an input port",
                edge.to.node, edge.to.port
            )));
        }
        if !compatible_data_types(&from.data_type, &to.data_type) {
            errors.push(ProjectValidationError::new(format!(
                "incompatible edge {}:{} {} -> {}:{} {}",
                edge.from.node,
                edge.from.port,
                type_label(&from.data_type),
                edge.to.node,
                edge.to.port,
                type_label(&to.data_type)
            )));
        }
    }
}

pub(crate) fn detect_cycle(graph: &GraphDocument) -> Option<Vec<String>> {
    let mut state: HashMap<&str, VisitState> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), VisitState::Unvisited))
        .collect();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        adjacency
            .entry(edge.from.node.as_str())
            .or_default()
            .push(edge.to.node.as_str());
    }

    for node in &graph.nodes {
        let mut stack = Vec::new();
        if visit(node.id.as_str(), &adjacency, &mut state, &mut stack).is_some() {
            return Some(stack.into_iter().map(str::to_owned).collect());
        }
    }

    None
}

fn cycle_node_list(graph: &GraphDocument) -> String {
    detect_cycle(graph)
        .filter(|nodes| !nodes.is_empty())
        .unwrap_or_else(|| graph.nodes.iter().map(|node| node.id.clone()).collect())
        .join(", ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Unvisited,
    Visiting,
    Visited,
}

fn visit<'a>(
    node: &'a str,
    adjacency: &HashMap<&'a str, Vec<&'a str>>,
    state: &mut HashMap<&'a str, VisitState>,
    stack: &mut Vec<&'a str>,
) -> Option<()> {
    match state.get(node).copied() {
        Some(VisitState::Visiting) => {
            stack.push(node);
            Some(())
        }
        Some(VisitState::Visited) => None,
        _ => {
            state.insert(node, VisitState::Visiting);
            stack.push(node);
            for next in adjacency.get(node).into_iter().flatten().copied() {
                if state.contains_key(next) && visit(next, adjacency, state, stack).is_some() {
                    return Some(());
                }
            }
            stack.pop();
            state.insert(node, VisitState::Visited);
            None
        }
    }
}
