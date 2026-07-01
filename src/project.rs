use std::{collections::HashMap, error::Error, fmt};

use crate::{
    DataFlow, GraphDocument, GraphNode, NodeDefinition, NodeRegistry, Port, PortDirection,
    compatible_data_types, type_label, validate_graph_document,
};

const RENDER_FULLSCREEN_SHADER_KIND: &str = "object.core.render.fullscreen-shader";

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

    if let Some(nodes) = detect_cycle(graph) {
        errors.push(ProjectValidationError::new(format!(
            "cycle detected: {}",
            nodes.join(", ")
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
            if allows_dynamic_shader_input(node, definition, snapshot_port) {
                continue;
            }
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
                issue_type_label(&snapshot_port.data_type),
                issue_type_label(&definition_port.data_type)
            )));
        }
    }
}

fn validate_edges(
    graph: &GraphDocument,
    definitions_by_node: &HashMap<&str, &NodeDefinition>,
    errors: &mut Vec<ProjectValidationError>,
) {
    let nodes_by_id: HashMap<&str, &GraphNode> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    for edge in &graph.edges {
        let from_definition = definitions_by_node.get(edge.from.node.as_str());
        let to_definition = definitions_by_node.get(edge.to.node.as_str());
        let from = nodes_by_id
            .get(edge.from.node.as_str())
            .and_then(|node| node.ports.iter().find(|port| port.id == edge.from.port));
        let to = nodes_by_id
            .get(edge.to.node.as_str())
            .and_then(|node| node.ports.iter().find(|port| port.id == edge.to.port));

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
                issue_type_label(&from.data_type),
                edge.to.node,
                edge.to.port,
                issue_type_label(&to.data_type)
            )));
        }
    }
}

fn issue_type_label(data_type: &crate::DataType) -> String {
    match data_type.flow {
        DataFlow::Control => match data_type.data_kind.as_str() {
            "value.core.float32" => "value.core.float32".to_owned(),
            "value.core.int32" => "value.core.int32".to_owned(),
            "value.core.uint32" => "value.core.uint32".to_owned(),
            "value.core.message" => "value.core.message".to_owned(),
            "value.core.bool" => "value.core.bool".to_owned(),
            "value.core.string" => "value.core.string".to_owned(),
            "value.core.color" => "value.core.color".to_owned(),
            _ => type_label(data_type),
        },
        DataFlow::Event if data_type.data_kind == "value.core.bang" => "value.core.bang".to_owned(),
        DataFlow::Resource if data_type.data_kind == "value.core.tensor" => {
            "value.core.tensor".to_owned()
        }
        _ => type_label(data_type),
    }
}

fn allows_dynamic_shader_input(node: &GraphNode, definition: &NodeDefinition, port: &Port) -> bool {
    node.kind == RENDER_FULLSCREEN_SHADER_KIND
        && definition.id == RENDER_FULLSCREEN_SHADER_KIND
        && port.direction == PortDirection::Input
        && port.data_type.flow == DataFlow::Control
        && matches!(
            port.data_type.data_kind.as_str(),
            "value.core.float32" | "value.core.int32" | "value.core.bool" | "value.core.color"
        )
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn graph(value: Value) -> GraphDocument {
        serde_json::from_value(value).expect("graph fixture should deserialize")
    }

    fn definition(value: Value) -> NodeDefinition {
        serde_json::from_value(value).expect("definition fixture should deserialize")
    }

    fn registry(definitions: Vec<NodeDefinition>) -> NodeRegistry {
        let mut registry = NodeRegistry::new();
        for definition in definitions {
            registry
                .insert(definition)
                .expect("definition fixture should be valid");
        }
        registry
    }

    fn value_source_definition() -> NodeDefinition {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.source",
          "version": "0.1.0",
          "displayName": "Source",
          "category": "Core",
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
          ],
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn value_target_definition() -> NodeDefinition {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.float",
          "version": "0.1.0",
          "displayName": "Target",
          "category": "Core",
          "ports": [
            { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" }
          ],
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn base_graph() -> GraphDocument {
        graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "project",
          "revision": "1",
          "nodes": [
            {
              "id": "source",
              "kind": "object.core.source",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            },
            {
              "id": "target",
              "kind": "object.core.float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" }
              ]
            }
          ],
          "edges": [
            { "from": { "node": "source", "port": "out" }, "to": { "node": "target", "port": "in" } }
          ]
        }))
    }

    #[test]
    fn validates_project_against_registry() {
        let graph = base_graph();
        let registry = registry(vec![value_source_definition(), value_target_definition()]);

        assert!(validate_project(&graph, &registry).is_ok());
    }

    #[test]
    fn reports_graph_schema_and_missing_definition_errors() {
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "broken",
          "revision": "1",
          "nodes": [
            {
              "id": "dup",
              "kind": "object.core.source",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            },
            {
              "id": "dup",
              "kind": "object.core.missing",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            }
          ],
          "edges": []
        }));

        let report = validate_project(&graph, &NodeRegistry::new()).unwrap_err();
        let display = report.to_string();

        assert!(report.errors().len() >= 3);
        assert!(display.contains("duplicate node id: dup"));
        assert!(display.contains("missing node definition: object.core.source@0.1.0"));
        assert!(display.contains("missing node definition: object.core.missing@0.1.0"));
    }

    #[test]
    fn reports_snapshot_manifest_mismatches() {
        let definition = definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.snapshot",
          "version": "0.1.0",
          "displayName": "Snapshot",
          "category": "Core",
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32", "range": { "min": 0, "max": 1 } } },
            { "id": "unused", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
          ],
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }));
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "snapshot",
          "revision": "1",
          "nodes": [
            {
              "id": "node",
              "kind": "object.core.snapshot",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "input", "type": { "flow": "event", "dataKind": "value.core.bang" }, "activation": "trigger" },
                { "id": "ghost", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            }
          ],
          "edges": []
        }));
        let report = validate_project(&graph, &registry(vec![definition])).unwrap_err();
        let display = report.to_string();

        assert!(display.contains("port snapshot missing manifest port: node.unused"));
        assert!(display.contains("port snapshot references missing manifest port: node.ghost"));
        assert!(display.contains("direction Input != definition direction Output"));
        assert!(display.contains("flow Event != definition flow Control"));
        assert!(
            display.contains("dataKind value.core.bang != definition dataKind value.core.float32")
        );
        assert!(
            display.contains(
                "value.core.bang is not compatible with definition type value.core.float32"
            )
        );
    }

    #[test]
    fn accepts_dynamic_fullscreen_shader_value_input_snapshots() {
        let definition = definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.render.fullscreen-shader",
          "version": "0.1.0",
          "displayName": "Fullscreen Shader",
          "category": "Render",
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "resource", "dataKind": "value.core.tensor", "format": "rgba8unorm", "colorSpace": "srgb" } }
          ],
          "execution": { "model": "gpu_pass" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }));
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "dynamic-shader",
          "revision": "1",
          "nodes": [
            {
              "id": "shader",
              "kind": "object.core.render.fullscreen-shader",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "speed", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
                { "id": "enabled", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.bool" }, "activation": "latched" },
                { "id": "out", "direction": "output", "type": { "flow": "resource", "dataKind": "value.core.tensor", "format": "rgba8unorm", "colorSpace": "srgb" } }
              ]
            }
          ],
          "edges": []
        }));

        validate_project(&graph, &registry(vec![definition]))
            .expect("dynamic shader input snapshots should validate");
    }

    #[test]
    fn reports_edge_endpoint_direction_and_type_errors() {
        let source_definition = definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.edge-source",
          "version": "0.1.0",
          "displayName": "Edge Source",
          "category": "Core",
          "ports": [
            { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
          ],
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }));
        let target_definition = definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.edge-target",
          "version": "0.1.0",
          "displayName": "Edge Target",
          "category": "Core",
          "ports": [
            { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.bool" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
          ],
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }));
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "edges",
          "revision": "1",
          "nodes": [
            {
              "id": "source",
              "kind": "object.core.edge-source",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            },
            {
              "id": "target",
              "kind": "object.core.edge-target",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.bool" }, "activation": "latched" },
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            }
          ],
          "edges": [
            { "from": { "node": "source", "port": "missing" }, "to": { "node": "target", "port": "missing" } },
            { "from": { "node": "source", "port": "in" }, "to": { "node": "target", "port": "out" } },
            { "from": { "node": "source", "port": "out" }, "to": { "node": "target", "port": "in" } }
          ]
        }));

        let report = validate_project(
            &graph,
            &registry(vec![source_definition, target_definition]),
        )
        .unwrap_err();
        let display = report.to_string();

        assert!(display.contains("edge references missing manifest source port source:missing"));
        assert!(display.contains("edge references missing manifest target port target:missing"));
        assert!(display.contains("edge source source:in is not an output port"));
        assert!(display.contains("edge target target:out is not an input port"));
        assert!(display.contains(
            "incompatible edge source:out value.core.float32 -> target:in value.core.bool"
        ));
    }

    #[test]
    fn reports_cycles() {
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "cycle",
          "revision": "1",
          "nodes": [
            {
              "id": "a",
              "kind": "object.core.float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            },
            {
              "id": "b",
              "kind": "object.core.float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            }
          ],
          "edges": [
            { "from": { "node": "a", "port": "out" }, "to": { "node": "b", "port": "in" } },
            { "from": { "node": "b", "port": "out" }, "to": { "node": "a", "port": "in" } }
          ]
        }));
        let pass_definition = definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.float",
          "version": "0.1.0",
          "displayName": "Target",
          "category": "Core",
          "ports": [
            { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
            { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
          ],
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }));

        let report = validate_project(&graph, &registry(vec![pass_definition])).unwrap_err();

        assert!(report.to_string().contains("cycle detected: a, b, a"));
    }
}
