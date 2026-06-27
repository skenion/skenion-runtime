use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::object_text::{is_payload_identity_kind, resolve_object_text_v01};
use crate::{
    CycleValidationCurrent, DataFlow, DataType, EdgeEndpointCurrent, EdgeSpecCurrent,
    ExecutionGroup, ExecutionModel, ExecutionModelCurrent, FanOutPolicyCurrent,
    GraphDocumentCurrent, GraphNodeCurrent, GraphValidationResultCurrent, MergePolicyCurrent,
    NodeDefinitionCurrent, PatchDefinitionCurrent, PlanEdge, PlanEdgeMetadata, PlanNode,
    PortDirectionCurrent, PortRateCurrent, PortSpecCurrent, ProjectDocumentCurrent,
    RuntimeDiagnostic, StringOrStrings, ViewState, compatible_data_types,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const SUBPATCH_KIND: &str = "object.core.subpatch";
const SUBPATCH_SHORTHAND_KIND: &str = "p";
const INLET_KIND: &str = "object.core.inlet";
const OUTLET_KIND: &str = "object.core.outlet";
const MAX_SUBPATCH_DEPTH: usize = 16;
pub const CURRENT_SCHEMA_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRequestCurrent {
    #[serde(skip)]
    pub document: Option<ProjectDocumentCurrent>,
    pub graph: GraphDocumentCurrent,
    #[serde(default)]
    pub nodes: Vec<NodeDefinitionCurrent>,
    #[serde(default)]
    pub patch_library: Vec<PatchDefinitionCurrent>,
    #[serde(default)]
    pub view_state: Option<ViewState>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunProjectRequestCurrent {
    #[serde(skip)]
    pub document: Option<ProjectDocumentCurrent>,
    pub graph: GraphDocumentCurrent,
    #[serde(default)]
    pub nodes: Vec<NodeDefinitionCurrent>,
    #[serde(default)]
    pub patch_library: Vec<PatchDefinitionCurrent>,
    #[serde(default)]
    pub view_state: Option<ViewState>,
    pub frames: Option<usize>,
}

impl From<ProjectDocumentCurrent> for ProjectRequestCurrent {
    fn from(document: ProjectDocumentCurrent) -> Self {
        Self {
            graph: document.graph.clone(),
            nodes: Vec::new(),
            patch_library: document.patch_library.clone(),
            view_state: Some(document.view_state.clone()),
            document: Some(document),
        }
    }
}

impl From<ProjectDocumentCurrent> for RunProjectRequestCurrent {
    fn from(document: ProjectDocumentCurrent) -> Self {
        Self {
            graph: document.graph.clone(),
            nodes: Vec::new(),
            patch_library: document.patch_library.clone(),
            view_state: Some(document.view_state.clone()),
            document: Some(document),
            frames: None,
        }
    }
}

impl ProjectRequestCurrent {
    pub fn from_project_document(
        document: ProjectDocumentCurrent,
        nodes: Vec<NodeDefinitionCurrent>,
    ) -> Self {
        Self {
            graph: document.graph.clone(),
            nodes,
            patch_library: document.patch_library.clone(),
            view_state: Some(document.view_state.clone()),
            document: Some(document),
        }
    }
}

impl RunProjectRequestCurrent {
    pub fn from_project_document(
        document: ProjectDocumentCurrent,
        nodes: Vec<NodeDefinitionCurrent>,
        frames: Option<usize>,
    ) -> Self {
        Self {
            graph: document.graph.clone(),
            nodes,
            patch_library: document.patch_library.clone(),
            view_state: Some(document.view_state.clone()),
            document: Some(document),
            frames,
        }
    }
}

type CurrentValidation =
    Result<(Vec<RuntimeDiagnostic>, GraphValidationResultCurrent), Vec<RuntimeDiagnostic>>;

#[derive(Debug, Clone)]
struct ExpandedGraphCurrent {
    nodes: Vec<GraphNodeCurrent>,
    edges: Vec<ExpansionEdge>,
    boundary_pins: HashSet<String>,
    inlets: HashMap<String, String>,
    outlets: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct ExpansionEdge {
    edge: EdgeSpecCurrent,
    source: ExpansionEndpoint,
    target: ExpansionEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExpansionEndpoint {
    Node(EdgeEndpointCurrent),
    Boundary(String),
}

#[derive(Debug, Clone)]
enum NodeExpansion {
    Node(String),
    Boundary(String),
    Subpatch {
        inlets: HashMap<String, String>,
        outlets: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, Copy)]
enum BoundaryKind {
    Inlet,
    Outlet,
}

struct ExpansionContext<'a> {
    patches: HashMap<&'a str, &'a PatchDefinitionCurrent>,
    diagnostics: Vec<RuntimeDiagnostic>,
}

pub fn expand_project_graph_current(
    graph: &GraphDocumentCurrent,
    patch_library: &[PatchDefinitionCurrent],
) -> Result<GraphDocumentCurrent, Vec<RuntimeDiagnostic>> {
    let mut context = ExpansionContext {
        patches: patch_library
            .iter()
            .map(|definition| (definition.id.as_str(), definition))
            .collect(),
        diagnostics: Vec::new(),
    };
    let expanded = expand_graph_current(graph, "", 0, &[], &mut context);

    if !context.diagnostics.is_empty() {
        return Err(context.diagnostics);
    }

    Ok(GraphDocumentCurrent {
        schema: graph.schema.clone(),
        schema_version: graph.schema_version.clone(),
        id: graph.id.clone(),
        revision: graph.revision.clone(),
        nodes: expanded.nodes,
        edges: contract_boundary_edges(expanded.edges, expanded.boundary_pins),
        cable_styles: graph.cable_styles.clone(),
    })
}

pub fn validate_project_request_current(request: &ProjectRequestCurrent) -> CurrentValidation {
    validate_patch_library_current(&request.patch_library)?;
    let graph = expand_project_graph_current(&request.graph, &request.patch_library)?;
    validate_project_current(&graph, &request.nodes)
}

pub fn build_execution_plan_request_current(
    request: &ProjectRequestCurrent,
) -> Result<(crate::ExecutionPlan, Vec<RuntimeDiagnostic>), Vec<RuntimeDiagnostic>> {
    validate_patch_library_current(&request.patch_library)?;
    let graph = expand_project_graph_current(&request.graph, &request.patch_library)?;
    build_execution_plan_current(&graph, &request.nodes)
}

pub fn build_execution_plan_run_request_current(
    request: &RunProjectRequestCurrent,
) -> Result<(crate::ExecutionPlan, Vec<RuntimeDiagnostic>), Vec<RuntimeDiagnostic>> {
    validate_patch_library_current(&request.patch_library)?;
    let graph = expand_project_graph_current(&request.graph, &request.patch_library)?;
    build_execution_plan_current(&graph, &request.nodes)
}

fn validate_patch_library_current(
    patch_library: &[PatchDefinitionCurrent],
) -> Result<(), Vec<RuntimeDiagnostic>> {
    let mut diagnostics = Vec::new();
    let mut seen = HashSet::new();

    for patch in patch_library {
        if !seen.insert(patch.id.as_str()) {
            diagnostics.push(RuntimeDiagnostic::structured_error(
                "subpatch.duplicate-patch-id",
                format!("duplicate patch id: {}", patch.id),
                json!({ "patchId": patch.id }),
            ));
        }

        if let Some(diagnostic) = schema_version_diagnostic_with_details(
            "graph",
            Some(patch.graph.schema_version.as_str()),
            json!({ "patchId": patch.id }),
        ) {
            diagnostics.push(diagnostic);
        }

        if let Err(report) = skenion_contracts::validate_patch_definition_v01(patch) {
            diagnostics.extend(
                report
                    .errors()
                    .iter()
                    .filter(|error| !is_schema_version_contract_error(&error.message))
                    .map(|error| {
                        RuntimeDiagnostic::structured_error(
                            "subpatch.invalid-patch-definition",
                            error.message.clone(),
                            json!({ "patchId": patch.id }),
                        )
                    }),
            );
        }

        for node in &patch.graph.nodes {
            if is_payload_identity_node_kind_current(&node.kind) {
                diagnostics.push(payload_identity_node_kind_diagnostic_current(
                    Some(&patch.id),
                    &patch.graph,
                    node,
                ));
            }
        }
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn expand_graph_current(
    graph: &GraphDocumentCurrent,
    namespace: &str,
    depth: usize,
    stack: &[String],
    context: &mut ExpansionContext<'_>,
) -> ExpandedGraphCurrent {
    let mut expanded = ExpandedGraphCurrent {
        nodes: Vec::new(),
        edges: Vec::new(),
        boundary_pins: HashSet::new(),
        inlets: HashMap::new(),
        outlets: HashMap::new(),
    };
    let mut node_expansions = HashMap::new();

    for node in &graph.nodes {
        if is_inlet_node(node) {
            let pin = register_boundary_node(
                node,
                namespace,
                BoundaryKind::Inlet,
                &mut expanded.boundary_pins,
                &mut expanded.inlets,
            );
            node_expansions.insert(node.id.clone(), NodeExpansion::Boundary(pin));
        } else if is_outlet_node(node) {
            let pin = register_boundary_node(
                node,
                namespace,
                BoundaryKind::Outlet,
                &mut expanded.boundary_pins,
                &mut expanded.outlets,
            );
            node_expansions.insert(node.id.clone(), NodeExpansion::Boundary(pin));
        } else if is_subpatch_node(node) {
            let Some(patch_ref) = subpatch_ref(node) else {
                context.diagnostics.push(subpatch_diagnostic(
                    "subpatch.missing-ref",
                    format!(
                        "subpatch node {} is missing a patch reference",
                        namespaced_id(namespace, &node.id)
                    ),
                    namespace,
                    node,
                    None,
                    depth,
                    stack,
                ));
                continue;
            };

            if stack.iter().any(|id| id == &patch_ref) {
                let mut path = stack.to_vec();
                path.push(patch_ref.clone());
                context.diagnostics.push(subpatch_diagnostic_with_path(
                    "subpatch.recursion",
                    format!(
                        "subpatch node {} recursively references patch definition {patch_ref}",
                        namespaced_id(namespace, &node.id)
                    ),
                    namespace,
                    node,
                    Some(&patch_ref),
                    depth + 1,
                    &path,
                ));
                continue;
            }

            if depth + 1 > MAX_SUBPATCH_DEPTH {
                let mut path = stack.to_vec();
                path.push(patch_ref.clone());
                context.diagnostics.push(subpatch_diagnostic_with_path(
                    "subpatch.depth-exceeded",
                    format!(
                        "subpatch node {} exceeds maximum expansion depth {MAX_SUBPATCH_DEPTH}",
                        namespaced_id(namespace, &node.id)
                    ),
                    namespace,
                    node,
                    Some(&patch_ref),
                    depth + 1,
                    &path,
                ));
                continue;
            }

            let Some(definition_graph) = context
                .patches
                .get(patch_ref.as_str())
                .map(|definition| definition.graph.clone())
            else {
                context.diagnostics.push(subpatch_diagnostic(
                    "subpatch.missing-patch",
                    format!(
                        "subpatch node {} references missing patch definition {patch_ref}",
                        namespaced_id(namespace, &node.id)
                    ),
                    namespace,
                    node,
                    Some(&patch_ref),
                    depth,
                    stack,
                ));
                continue;
            };

            let child_namespace = namespaced_id(namespace, &node.id);
            let mut child_stack = stack.to_vec();
            child_stack.push(patch_ref);
            let child = expand_graph_current(
                &definition_graph,
                &child_namespace,
                depth + 1,
                &child_stack,
                context,
            );
            expanded.nodes.extend(child.nodes);
            expanded.edges.extend(child.edges);
            expanded.boundary_pins.extend(child.boundary_pins);
            node_expansions.insert(
                node.id.clone(),
                NodeExpansion::Subpatch {
                    inlets: child.inlets,
                    outlets: child.outlets,
                },
            );
        } else {
            let namespaced = namespaced_id(namespace, &node.id);
            let mut cloned = node.clone();
            cloned.id = namespaced.clone();
            expanded.nodes.push(cloned);
            node_expansions.insert(node.id.clone(), NodeExpansion::Node(namespaced));
        }
    }

    for edge in &graph.edges {
        let source = resolve_source_endpoint(edge, namespace, &node_expansions, context);
        let target = resolve_target_endpoint(edge, namespace, &node_expansions, context);
        let mut cloned = edge.clone();
        cloned.id = namespaced_id(namespace, &edge.id);
        expanded.edges.push(ExpansionEdge {
            edge: cloned,
            source,
            target,
        });
    }

    expanded
}

fn contract_boundary_edges(
    mut edges: Vec<ExpansionEdge>,
    boundary_pins: HashSet<String>,
) -> Vec<EdgeSpecCurrent> {
    let mut merged_boundary_edge_index = 0usize;

    while let Some(pin) = boundary_pins
        .iter()
        .find(|pin| {
            has_incoming_boundary_edge(&edges, pin) && has_outgoing_boundary_edge(&edges, pin)
        })
        .cloned()
    {
        let incoming = edges
            .iter()
            .filter(|edge| matches!(&edge.target, ExpansionEndpoint::Boundary(candidate) if candidate == &pin))
            .cloned()
            .collect::<Vec<_>>();
        let outgoing = edges
            .iter()
            .filter(|edge| matches!(&edge.source, ExpansionEndpoint::Boundary(candidate) if candidate == &pin))
            .cloned()
            .collect::<Vec<_>>();
        let mut retained = edges
            .into_iter()
            .filter(|edge| !edge_touches_boundary_pin(edge, &pin))
            .collect::<Vec<_>>();

        for source_edge in &incoming {
            for target_edge in &outgoing {
                if source_edge.source == target_edge.target {
                    continue;
                }
                merged_boundary_edge_index += 1;
                retained.push(merge_boundary_edges(
                    source_edge,
                    target_edge,
                    &pin,
                    merged_boundary_edge_index,
                ));
            }
        }

        edges = retained;
    }

    edges
        .into_iter()
        .filter_map(expansion_edge_to_real_edge)
        .collect()
}

fn has_incoming_boundary_edge(edges: &[ExpansionEdge], pin: &str) -> bool {
    edges.iter().any(
        |edge| matches!(&edge.target, ExpansionEndpoint::Boundary(candidate) if candidate == pin),
    )
}

fn has_outgoing_boundary_edge(edges: &[ExpansionEdge], pin: &str) -> bool {
    edges.iter().any(
        |edge| matches!(&edge.source, ExpansionEndpoint::Boundary(candidate) if candidate == pin),
    )
}

fn edge_touches_boundary_pin(edge: &ExpansionEdge, pin: &str) -> bool {
    matches!(&edge.source, ExpansionEndpoint::Boundary(candidate) if candidate == pin)
        || matches!(&edge.target, ExpansionEndpoint::Boundary(candidate) if candidate == pin)
}

fn merge_boundary_edges(
    source_edge: &ExpansionEdge,
    target_edge: &ExpansionEdge,
    pin: &str,
    merged_boundary_edge_index: usize,
) -> ExpansionEdge {
    let mut edge = target_edge.edge.clone();
    edge.id = format!(
        "{}__{}__{}",
        source_edge.edge.id,
        boundary_id_fragment(pin),
        merged_boundary_edge_index
    );
    if edge.resolved_type.is_none() {
        edge.resolved_type = source_edge.edge.resolved_type.clone();
    }
    if edge.order.is_none() {
        edge.order = source_edge.edge.order;
    }
    if edge.enabled.is_none() {
        edge.enabled = source_edge.edge.enabled;
    }
    if edge.adapter.is_none() {
        edge.adapter = source_edge.edge.adapter.clone();
    }
    if edge.feedback.is_none() {
        edge.feedback = source_edge.edge.feedback.clone();
    }
    if edge.style_override.is_none() {
        edge.style_override = source_edge.edge.style_override.clone();
    }
    if edge.label.is_none() {
        edge.label = source_edge.edge.label.clone();
    }
    if edge.description.is_none() {
        edge.description = source_edge.edge.description.clone();
    }

    ExpansionEdge {
        edge,
        source: source_edge.source.clone(),
        target: target_edge.target.clone(),
    }
}

fn expansion_edge_to_real_edge(expansion: ExpansionEdge) -> Option<EdgeSpecCurrent> {
    let ExpansionEndpoint::Node(source) = expansion.source else {
        return None;
    };
    let ExpansionEndpoint::Node(target) = expansion.target else {
        return None;
    };
    let mut edge = expansion.edge;
    edge.source = source;
    edge.target = target;
    Some(edge)
}

fn resolve_source_endpoint(
    edge: &EdgeSpecCurrent,
    namespace: &str,
    nodes: &HashMap<String, NodeExpansion>,
    context: &mut ExpansionContext<'_>,
) -> ExpansionEndpoint {
    match nodes.get(&edge.source.node_id) {
        Some(NodeExpansion::Node(node_id)) => ExpansionEndpoint::Node(EdgeEndpointCurrent {
            node_id: node_id.clone(),
            port_id: edge.source.port_id.clone(),
        }),
        Some(NodeExpansion::Boundary(pin)) => ExpansionEndpoint::Boundary(pin.clone()),
        Some(NodeExpansion::Subpatch { outlets, .. }) => outlets
            .get(edge.source.port_id.as_str())
            .map(|pin| ExpansionEndpoint::Boundary(pin.clone()))
            .unwrap_or_else(|| {
                context.diagnostics.push(boundary_diagnostic(
                    "subpatch.missing-outlet",
                    format!(
                        "subpatch node {} has no outlet boundary for port {}",
                        namespaced_id(namespace, &edge.source.node_id),
                        edge.source.port_id
                    ),
                    namespace,
                    &edge.source.node_id,
                    &edge.source.port_id,
                    BoundaryKind::Outlet,
                ));
                ExpansionEndpoint::Node(EdgeEndpointCurrent {
                    node_id: namespaced_id(namespace, &edge.source.node_id),
                    port_id: edge.source.port_id.clone(),
                })
            }),
        None => ExpansionEndpoint::Node(EdgeEndpointCurrent {
            node_id: namespaced_id(namespace, &edge.source.node_id),
            port_id: edge.source.port_id.clone(),
        }),
    }
}

fn resolve_target_endpoint(
    edge: &EdgeSpecCurrent,
    namespace: &str,
    nodes: &HashMap<String, NodeExpansion>,
    context: &mut ExpansionContext<'_>,
) -> ExpansionEndpoint {
    match nodes.get(&edge.target.node_id) {
        Some(NodeExpansion::Node(node_id)) => ExpansionEndpoint::Node(EdgeEndpointCurrent {
            node_id: node_id.clone(),
            port_id: edge.target.port_id.clone(),
        }),
        Some(NodeExpansion::Boundary(pin)) => ExpansionEndpoint::Boundary(pin.clone()),
        Some(NodeExpansion::Subpatch { inlets, .. }) => inlets
            .get(edge.target.port_id.as_str())
            .map(|pin| ExpansionEndpoint::Boundary(pin.clone()))
            .unwrap_or_else(|| {
                context.diagnostics.push(boundary_diagnostic(
                    "subpatch.missing-inlet",
                    format!(
                        "subpatch node {} has no inlet boundary for port {}",
                        namespaced_id(namespace, &edge.target.node_id),
                        edge.target.port_id
                    ),
                    namespace,
                    &edge.target.node_id,
                    &edge.target.port_id,
                    BoundaryKind::Inlet,
                ));
                ExpansionEndpoint::Node(EdgeEndpointCurrent {
                    node_id: namespaced_id(namespace, &edge.target.node_id),
                    port_id: edge.target.port_id.clone(),
                })
            }),
        None => ExpansionEndpoint::Node(EdgeEndpointCurrent {
            node_id: namespaced_id(namespace, &edge.target.node_id),
            port_id: edge.target.port_id.clone(),
        }),
    }
}

fn register_boundary_node(
    node: &GraphNodeCurrent,
    namespace: &str,
    kind: BoundaryKind,
    boundary_pins: &mut HashSet<String>,
    aliases: &mut HashMap<String, String>,
) -> String {
    let key = boundary_key(node);
    let pin = format!(
        "{}@{}::{}",
        namespace_prefix(namespace),
        match kind {
            BoundaryKind::Inlet => "inlet",
            BoundaryKind::Outlet => "outlet",
        },
        key
    );
    boundary_pins.insert(pin.clone());

    for alias in boundary_aliases(node, &key) {
        match aliases.get(&alias) {
            Some(existing) if existing != &pin => {
                aliases.remove(&alias);
            }
            Some(_) => {}
            None => {
                aliases.insert(alias, pin.clone());
            }
        }
    }

    pin
}

fn boundary_aliases(node: &GraphNodeCurrent, key: &str) -> Vec<String> {
    let mut aliases = vec![key.to_owned(), node.id.clone()];
    for param_key in ["portId", "port", "name", "id", "label"] {
        if let Some(alias) = string_param(&node.params, param_key) {
            aliases.push(alias);
        }
    }
    if node.ports.len() == 1 {
        aliases.push(node.ports[0].id.clone());
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn boundary_key(node: &GraphNodeCurrent) -> String {
    ["portId", "port", "name", "id", "label"]
        .into_iter()
        .find_map(|key| string_param(&node.params, key))
        .unwrap_or_else(|| node.id.clone())
}

fn subpatch_ref(node: &GraphNodeCurrent) -> Option<String> {
    ["patchRef", "patchId", "patch", "ref", "name", "id"]
        .into_iter()
        .find_map(|key| string_param(&node.params, key))
        .or_else(|| subpatch_object_text(node).and_then(|text| parse_subpatch_object_text(&text)))
}

fn parse_subpatch_object_text(text: &str) -> Option<String> {
    let resolution = resolve_object_text_v01(text);
    if resolution.resolved_kind.as_deref() != Some(SUBPATCH_KIND) || !resolution.ok() {
        return None;
    }
    resolution
        .params
        .get("patchRef")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn subpatch_object_text(node: &GraphNodeCurrent) -> Option<String> {
    node.object_text.clone().or_else(|| {
        ["objectText", "sourceText", "text"]
            .into_iter()
            .find_map(|key| string_param(&node.params, key))
    })
}

fn node_object_text(node: &GraphNodeCurrent) -> Option<String> {
    node.object_text.clone().or_else(|| {
        ["objectText", "sourceText"]
            .into_iter()
            .find_map(|key| string_param(&node.params, key))
    })
}

fn string_param(params: &Map<String, Value>, key: &str) -> Option<String> {
    match params.get(key)? {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn is_subpatch_node(node: &GraphNodeCurrent) -> bool {
    matches!(node.kind.as_str(), SUBPATCH_KIND | SUBPATCH_SHORTHAND_KIND)
}

fn is_inlet_node(node: &GraphNodeCurrent) -> bool {
    node.kind == INLET_KIND
}

fn is_outlet_node(node: &GraphNodeCurrent) -> bool {
    node.kind == OUTLET_KIND
}

fn namespaced_id(namespace: &str, id: &str) -> String {
    if namespace.is_empty() {
        id.to_owned()
    } else {
        format!("{namespace}::{id}")
    }
}

fn namespace_prefix(namespace: &str) -> String {
    if namespace.is_empty() {
        String::new()
    } else {
        format!("{namespace}::")
    }
}

fn boundary_id_fragment(pin: &str) -> String {
    pin.chars()
        .map(|character| match character {
            ':' | '@' | '/' | '\\' | ' ' => '_',
            _ => character,
        })
        .collect()
}

fn subpatch_diagnostic(
    code: &'static str,
    message: String,
    namespace: &str,
    node: &GraphNodeCurrent,
    patch_ref: Option<&str>,
    depth: usize,
    stack: &[String],
) -> RuntimeDiagnostic {
    subpatch_diagnostic_with_path(code, message, namespace, node, patch_ref, depth, stack)
}

fn subpatch_diagnostic_with_path(
    code: &'static str,
    message: String,
    namespace: &str,
    node: &GraphNodeCurrent,
    patch_ref: Option<&str>,
    depth: usize,
    path: &[String],
) -> RuntimeDiagnostic {
    RuntimeDiagnostic::structured_error(
        code,
        message,
        json!({
            "nodeId": namespaced_id(namespace, &node.id),
            "kind": node.kind.as_str(),
            "patchRef": patch_ref,
            "depth": depth,
            "path": path,
        }),
    )
}

fn boundary_diagnostic(
    code: &'static str,
    message: String,
    namespace: &str,
    node_id: &str,
    port_id: &str,
    kind: BoundaryKind,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic::structured_error(
        code,
        message,
        json!({
            "nodeId": namespaced_id(namespace, node_id),
            "portId": port_id,
            "boundary": match kind {
                BoundaryKind::Inlet => "inlet",
                BoundaryKind::Outlet => "outlet",
            },
        }),
    )
}

pub fn validate_project_current(
    graph: &GraphDocumentCurrent,
    nodes: &[NodeDefinitionCurrent],
) -> CurrentValidation {
    let mut diagnostics = Vec::new();
    let mut registry: HashMap<(&str, &str), &NodeDefinitionCurrent> = HashMap::new();

    for definition in nodes {
        if is_payload_identity_node_kind_current(&definition.id) {
            diagnostics.push(payload_identity_node_definition_diagnostic_current(
                definition,
            ));
        }
        if let Err(report) = skenion_contracts::validate_node_definition_v01(definition) {
            diagnostics.extend(report.errors().iter().map(|error| {
                contract_validation_diagnostic(
                    "node-definition",
                    "node-definition.invalid-contract",
                    error.message.clone(),
                    &definition.schema_version,
                    json!({ "nodeDefinitionId": definition.id }),
                )
            }));
        }
        registry.insert(
            (definition.id.as_str(), definition.version.as_str()),
            definition,
        );
    }

    let graph_analysis = skenion_contracts::analyze_graph_document_v01(graph);
    let graph_analysis_error_messages = graph_analysis
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == "error")
        .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
        .collect::<HashSet<_>>();
    if let Err(report) = skenion_contracts::validate_graph_document_v01(graph) {
        diagnostics.extend(
            report
                .errors()
                .iter()
                .filter(|error| !graph_analysis_error_messages.contains(error.message.as_str()))
                .map(|error| {
                    contract_validation_diagnostic(
                        "graph",
                        "graph.invalid-contract",
                        error.message.clone(),
                        &graph.schema_version,
                        json!({ "graphId": graph.id }),
                    )
                }),
        );
    }
    diagnostics.extend(
        graph_analysis
            .diagnostics
            .iter()
            .map(|diagnostic| graph_analysis_diagnostic_current(graph, diagnostic)),
    );

    for node in &graph.nodes {
        diagnostics.extend(object_text_diagnostics_current(graph, node));
        if is_payload_identity_node_kind_current(&node.kind) {
            diagnostics.push(payload_identity_node_kind_diagnostic_current(
                None, graph, node,
            ));
        }
        match registry.get(&(node.kind.as_str(), node.kind_version.as_str())) {
            Some(definition) => validate_node_snapshot_current(node, definition, &mut diagnostics),
            None => diagnostics.push(RuntimeDiagnostic::structured_error(
                "node-definition.missing",
                format!(
                    "missing node definition: {}@{}",
                    node.kind, node.kind_version
                ),
                json!({
                    "surface": "node-definition",
                    "nodeId": node.id,
                    "kind": node.kind,
                    "kindVersion": node.kind_version,
                }),
            )),
        }
    }
    validate_edges_current(graph, &mut diagnostics);

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == crate::DiagnosticSeverity::Error)
    {
        Err(diagnostics)
    } else {
        Ok((diagnostics, graph_analysis))
    }
}

pub(crate) fn is_payload_identity_node_kind_current(kind: &str) -> bool {
    is_payload_identity_kind(kind)
}

fn object_text_diagnostics_current(
    graph: &GraphDocumentCurrent,
    node: &GraphNodeCurrent,
) -> Vec<RuntimeDiagnostic> {
    let Some(object_text) = node_object_text(node) else {
        return Vec::new();
    };
    let resolution = resolve_object_text_v01(&object_text);
    let mut diagnostics = resolution
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code != "object-text.unresolved")
        .map(|diagnostic| {
            RuntimeDiagnostic::structured_error(
                diagnostic.code.clone(),
                diagnostic.message.clone(),
                json!({
                    "surface": "object-text",
                    "graphId": graph.id,
                    "nodeId": node.id,
                    "kind": node.kind,
                    "objectText": object_text,
                    "classSymbol": resolution.class_symbol,
                }),
            )
        })
        .collect::<Vec<_>>();

    if diagnostics.is_empty()
        && let Some(resolved_kind) = resolution.resolved_kind.as_deref()
        && resolved_kind != node.kind
        && node.kind != "object.core.unresolved"
    {
        diagnostics.push(RuntimeDiagnostic::structured_error(
            "object-text.kind-mismatch",
            format!(
                "object text {} resolves to {}, but node {} uses kind {}",
                object_text, resolved_kind, node.id, node.kind
            ),
            json!({
                "surface": "object-text",
                "graphId": graph.id,
                "nodeId": node.id,
                "objectText": object_text,
                "classSymbol": resolution.class_symbol,
                "resolvedKind": resolved_kind,
                "nodeKind": node.kind,
            }),
        ));
    }

    diagnostics
}

fn payload_identity_node_kind_diagnostic_current(
    patch_id: Option<&str>,
    graph: &GraphDocumentCurrent,
    node: &GraphNodeCurrent,
) -> RuntimeDiagnostic {
    let mut details = json!({
        "surface": "graph-node",
        "graphId": graph.id,
        "nodeId": node.id,
        "kind": node.kind,
        "kindVersion": node.kind_version,
    });
    if let Some(patch_id) = patch_id {
        details["patchId"] = json!(patch_id);
    }

    RuntimeDiagnostic::structured_error(
        "graph.payload-node-kind",
        format!(
            "node {} uses payload identity {} as an executable kind",
            node.id, node.kind
        ),
        details,
    )
}

fn payload_identity_node_definition_diagnostic_current(
    definition: &NodeDefinitionCurrent,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic::structured_error(
        "node-definition.payload-identity-id",
        format!("payload identity node definition id: {}", definition.id),
        json!({
            "surface": "node-definition",
            "nodeDefinitionId": definition.id,
            "version": definition.version,
        }),
    )
}

fn graph_analysis_diagnostic_current(
    graph: &GraphDocumentCurrent,
    diagnostic: &skenion_contracts::GraphValidationDiagnosticV01,
) -> RuntimeDiagnostic {
    let code = graph_analysis_runtime_code_current(&diagnostic.code);
    let details = json!({
        "surface": "graph",
        "graphId": graph.id,
        "nodes": diagnostic.nodes,
        "edges": diagnostic.edges,
    });

    match diagnostic.severity.as_str() {
        "warning" => {
            RuntimeDiagnostic::structured_warning(code, diagnostic.message.clone(), details)
        }
        "info" => RuntimeDiagnostic {
            severity: crate::DiagnosticSeverity::Info,
            message: diagnostic.message.clone(),
            code: Some(code),
            details: Some(details),
        },
        _ => RuntimeDiagnostic::structured_error(code, diagnostic.message.clone(), details),
    }
}

fn graph_analysis_runtime_code_current(code: &str) -> String {
    match code {
        "missing-source-port" => "graph.edge-missing-source-port",
        "missing-target-port" => "graph.edge-missing-target-port",
        "invalid-source-direction" => "graph.edge-source-direction",
        "invalid-target-direction" => "graph.edge-target-direction",
        "incompatible-type" => "graph.edge-incompatible-type",
        "payload-node-kind" => "graph.payload-node-kind",
        other => return format!("graph.{other}"),
    }
    .to_owned()
}

fn contract_validation_diagnostic(
    surface: &'static str,
    code: &'static str,
    message: String,
    received_schema_version: &str,
    mut details: Value,
) -> RuntimeDiagnostic {
    let object = details
        .as_object_mut()
        .expect("contract validation diagnostic details should be an object");
    object.insert("surface".to_owned(), json!(surface));
    object.insert(
        "expectedSchemaVersion".to_owned(),
        json!(CURRENT_SCHEMA_VERSION),
    );
    object.insert(
        "receivedSchemaVersion".to_owned(),
        json!(received_schema_version),
    );
    RuntimeDiagnostic::structured_error(code, message, details)
}

pub fn schema_version_diagnostic(
    surface: &'static str,
    received_schema_version: Option<&str>,
) -> Option<RuntimeDiagnostic> {
    schema_version_diagnostic_with_details(surface, received_schema_version, json!({}))
}

fn schema_version_diagnostic_with_details(
    surface: &'static str,
    received_schema_version: Option<&str>,
    mut details: Value,
) -> Option<RuntimeDiagnostic> {
    let object = details
        .as_object_mut()
        .expect("schema version diagnostic details should be an object");
    object.insert("surface".to_owned(), json!(surface));
    object.insert(
        "expectedSchemaVersion".to_owned(),
        json!(CURRENT_SCHEMA_VERSION),
    );
    object.insert(
        "receivedSchemaVersion".to_owned(),
        received_schema_version.map_or(Value::Null, Value::from),
    );

    match received_schema_version {
        Some(CURRENT_SCHEMA_VERSION) => None,
        Some(version) => Some(RuntimeDiagnostic::structured_error(
            "project.unsupported-schema-version",
            format!("unsupported {surface}.schemaVersion: {version}"),
            details,
        )),
        None => Some(RuntimeDiagnostic::structured_error(
            "project.missing-schema-version",
            format!("missing {surface}.schemaVersion in project request"),
            details,
        )),
    }
}

pub fn project_document_validation_diagnostics_current(
    document: &ProjectDocumentCurrent,
    report: &skenion_contracts::ValidationReportV01,
) -> Vec<RuntimeDiagnostic> {
    let mut diagnostics = Vec::new();
    if let Some(diagnostic) =
        schema_version_diagnostic("project", Some(document.schema_version.as_str()))
    {
        diagnostics.push(diagnostic);
    }
    if let Some(diagnostic) =
        schema_version_diagnostic("graph", Some(document.graph.schema_version.as_str()))
    {
        diagnostics.push(diagnostic);
    }
    for patch in &document.patch_library {
        if let Some(diagnostic) = schema_version_diagnostic_with_details(
            "graph",
            Some(patch.graph.schema_version.as_str()),
            json!({ "patchId": patch.id }),
        ) {
            diagnostics.push(diagnostic);
        }
    }

    diagnostics.extend(
        report
            .errors()
            .iter()
            .filter(|error| !is_schema_version_contract_error(&error.message))
            .map(|error| {
                RuntimeDiagnostic::structured_error(
                    "project.invalid-0.1",
                    error.message.clone(),
                    json!({ "projectId": document.id }),
                )
            }),
    );
    diagnostics
}

pub fn project_document_payload_schema_diagnostics(value: &Value) -> Vec<RuntimeDiagnostic> {
    let mut diagnostics = Vec::new();
    if let Some(diagnostic) = schema_version_diagnostic(
        "project",
        value.get("schemaVersion").and_then(Value::as_str),
    ) {
        diagnostics.push(diagnostic);
    }
    if let Some(diagnostic) = schema_version_diagnostic(
        "graph",
        value
            .get("graph")
            .and_then(|graph| graph.get("schemaVersion"))
            .and_then(Value::as_str),
    ) {
        diagnostics.push(diagnostic);
    }
    if let Some(patches) = value.get("patchLibrary").and_then(Value::as_array) {
        for patch in patches {
            let patch_id = patch.get("id").and_then(Value::as_str);
            if let Some(diagnostic) = schema_version_diagnostic_with_details(
                "graph",
                patch
                    .get("graph")
                    .and_then(|graph| graph.get("schemaVersion"))
                    .and_then(Value::as_str),
                json!({ "patchId": patch_id }),
            ) {
                diagnostics.push(diagnostic);
            }
        }
    }
    diagnostics
}

fn is_schema_version_contract_error(message: &str) -> bool {
    message.contains("expected schemaVersion 0.1.0, found")
}

pub fn build_execution_plan_current(
    graph: &GraphDocumentCurrent,
    nodes: &[NodeDefinitionCurrent],
) -> Result<(crate::ExecutionPlan, Vec<RuntimeDiagnostic>), Vec<RuntimeDiagnostic>> {
    let (diagnostics, analysis) = validate_project_current(graph, nodes)?;
    let registry = nodes
        .iter()
        .map(|definition| {
            (
                (definition.id.as_str(), definition.version.as_str()),
                definition,
            )
        })
        .collect::<HashMap<_, _>>();
    let ordered_node_ids = topological_order_current(graph);
    let graph_nodes = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut groups_by_model: BTreeMap<String, ExecutionGroup> = BTreeMap::new();
    let mut plan_nodes = Vec::new();

    for (order, node_id) in ordered_node_ids.iter().enumerate() {
        let node = graph_nodes
            .get(node_id.as_str())
            .expect("current 0.1 planning order should only contain graph nodes");
        let definition = registry
            .get(&(node.kind.as_str(), node.kind_version.as_str()))
            .expect("current 0.1 validation should resolve definitions");
        let execution_model = map_execution_model_current(&definition.execution.model);
        plan_nodes.push(PlanNode {
            node_id: node.id.clone(),
            kind: node.kind.clone(),
            kind_version: node.kind_version.clone(),
            execution_model: execution_model.clone(),
            order,
        });
        groups_by_model
            .entry(format!("{execution_model:?}"))
            .or_insert_with(|| ExecutionGroup {
                execution_model: execution_model.clone(),
                node_ids: Vec::new(),
            })
            .node_ids
            .push(node.id.clone());
    }

    Ok((
        crate::ExecutionPlan {
            graph_id: graph.id.clone(),
            graph_revision: graph.revision.clone(),
            nodes: plan_nodes,
            edges: graph
                .edges
                .iter()
                .map(|edge| plan_edge_current(graph, edge, &analysis))
                .collect(),
            groups: groups_by_model.into_values().collect(),
        },
        diagnostics,
    ))
}

fn topological_order_current(graph: &GraphDocumentCurrent) -> Vec<String> {
    let mut indegree: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), 0usize))
        .collect();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();

    for edge in graph.edges.iter().filter(|edge| !is_feedback_edge(edge)) {
        if indegree.contains_key(edge.source.node_id.as_str())
            && indegree.contains_key(edge.target.node_id.as_str())
        {
            adjacency
                .entry(edge.source.node_id.as_str())
                .or_default()
                .push(edge.target.node_id.as_str());
            *indegree
                .get_mut(edge.target.node_id.as_str())
                .expect("target node exists") += 1;
        }
    }

    let mut queue = graph
        .nodes
        .iter()
        .filter(|node| indegree.get(node.id.as_str()).copied() == Some(0))
        .map(|node| node.id.as_str())
        .collect::<VecDeque<_>>();
    let mut ordered = Vec::new();

    while let Some(node_id) = queue.pop_front() {
        ordered.push(node_id.to_owned());
        for next in adjacency.get(node_id).into_iter().flatten().copied() {
            let next_indegree = indegree.get_mut(next).expect("adjacent node exists");
            *next_indegree -= 1;
            if *next_indegree == 0 {
                queue.push_back(next);
            }
        }
    }

    if ordered.len() == graph.nodes.len() {
        return ordered;
    }

    let ordered_set = ordered.iter().cloned().collect::<HashSet<_>>();
    ordered.extend(
        graph
            .nodes
            .iter()
            .filter(|node| !ordered_set.contains(node.id.as_str()))
            .map(|node| node.id.clone()),
    );
    ordered
}

fn is_feedback_edge(edge: &EdgeSpecCurrent) -> bool {
    edge.feedback
        .as_ref()
        .is_some_and(|feedback| feedback.enabled)
}

fn validate_node_snapshot_current(
    node: &crate::GraphNodeCurrent,
    definition: &NodeDefinitionCurrent,
    diagnostics: &mut Vec<RuntimeDiagnostic>,
) {
    let definition_ports = definition
        .ports
        .iter()
        .map(|port| (port.id.as_str(), port))
        .collect::<HashMap<_, _>>();
    let snapshot_ports = node
        .ports
        .iter()
        .map(|port| (port.id.as_str(), port))
        .collect::<HashMap<_, _>>();

    for definition_port in &definition.ports {
        if !snapshot_ports.contains_key(definition_port.id.as_str()) {
            diagnostics.push(node_snapshot_diagnostic(
                "node.port-snapshot.missing-manifest-port",
                format!(
                    "port snapshot missing manifest port: {}.{}",
                    node.id, definition_port.id
                ),
                node,
                definition_port.id.as_str(),
            ));
        }
    }

    for snapshot_port in &node.ports {
        let Some(definition_port) = definition_ports.get(snapshot_port.id.as_str()) else {
            diagnostics.push(node_snapshot_diagnostic(
                "node.port-snapshot.unknown-manifest-port",
                format!(
                    "port snapshot references missing manifest port: {}.{}",
                    node.id, snapshot_port.id
                ),
                node,
                snapshot_port.id.as_str(),
            ));
            continue;
        };

        if snapshot_port.direction != definition_port.direction {
            diagnostics.push(node_snapshot_diagnostic(
                "node.port-snapshot.direction-mismatch",
                format!(
                    "port snapshot mismatch: {}.{} direction differs from definition",
                    node.id, snapshot_port.id
                ),
                node,
                snapshot_port.id.as_str(),
            ));
        }
        if snapshot_port.port_type != definition_port.port_type {
            diagnostics.push(node_snapshot_diagnostic(
                "node.port-snapshot.type-mismatch",
                format!(
                    "port snapshot mismatch: {}.{} type {} != definition type {}",
                    node.id, snapshot_port.id, snapshot_port.port_type, definition_port.port_type
                ),
                node,
                snapshot_port.id.as_str(),
            ));
        }
    }
}

fn validate_edges_current(graph: &GraphDocumentCurrent, diagnostics: &mut Vec<RuntimeDiagnostic>) {
    for edge in &graph.edges {
        let Some(source) = find_port(graph, &edge.source.node_id, &edge.source.port_id) else {
            continue;
        };
        let Some(target) = find_port(graph, &edge.target.node_id, &edge.target.port_id) else {
            continue;
        };

        if source.direction != PortDirectionCurrent::Output {
            diagnostics.push(edge_diagnostic(
                "graph.edge-source-direction",
                format!(
                    "edge source {}:{} is not an output port",
                    edge.source.node_id, edge.source.port_id
                ),
                edge,
            ));
        }
        if target.direction != PortDirectionCurrent::Input {
            diagnostics.push(edge_diagnostic(
                "graph.edge-target-direction",
                format!(
                    "edge target {}:{} is not an input port",
                    edge.target.node_id, edge.target.port_id
                ),
                edge,
            ));
        }

        let source_type = data_type_from_port_spec_current(source);
        let target_type = data_type_from_port_spec_current(target);
        if !compatible_data_types(&source_type, &target_type) {
            diagnostics.push(edge_diagnostic(
                "graph.edge-incompatible-type",
                format!(
                    "incompatible edge {}:{} {} -> {}:{} {}",
                    edge.source.node_id,
                    edge.source.port_id,
                    source.port_type,
                    edge.target.node_id,
                    edge.target.port_id,
                    target.port_type
                ),
                edge,
            ));
        }
    }
}

fn edge_diagnostic(
    code: &'static str,
    message: String,
    edge: &EdgeSpecCurrent,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic::structured_error(
        code,
        message,
        json!({
            "surface": "graph-edge",
            "edgeId": edge.id,
            "source": {
                "nodeId": edge.source.node_id,
                "portId": edge.source.port_id,
            },
            "target": {
                "nodeId": edge.target.node_id,
                "portId": edge.target.port_id,
            },
        }),
    )
}

fn data_type_from_port_spec_current(port: &PortSpecCurrent) -> DataType {
    let (canonical_flow, data_kind) = current_port_type_parts(&port.port_type);
    let format = match data_kind.as_str() {
        "value.core.float32" => Some(StringOrStrings::One("f32".to_owned())),
        "value.core.tensor" => Some(StringOrStrings::One("rgba8unorm".to_owned())),
        _ => None,
    };
    let color_space = (data_kind == "value.core.tensor").then(|| "srgb".to_owned());
    DataType {
        flow: canonical_flow.unwrap_or_else(|| match port.rate {
            Some(PortRateCurrent::Event) => DataFlow::Event,
            Some(PortRateCurrent::Audio) => DataFlow::Signal,
            Some(PortRateCurrent::Resource) | Some(PortRateCurrent::Io) => DataFlow::Resource,
            Some(PortRateCurrent::Control | PortRateCurrent::Render | PortRateCurrent::Gpu)
            | None => {
                if data_kind == "value.core.tensor" {
                    DataFlow::Resource
                } else {
                    DataFlow::Control
                }
            }
        }),
        data_kind,
        unit: None,
        range: None,
        shape: None,
        channels: None,
        sample_rate: None,
        format,
        color_space,
        frame_rate: None,
        alpha_policy: None,
        values: None,
    }
}

fn current_port_type_parts(port_type: &str) -> (Option<DataFlow>, String) {
    match port_type {
        value_type if value_type.starts_with("value.") => (None, value_type.to_owned()),
        other => (None, other.to_owned()),
    }
}

fn node_snapshot_diagnostic(
    code: &'static str,
    message: String,
    node: &crate::GraphNodeCurrent,
    port_id: &str,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic::structured_error(
        code,
        message,
        json!({
            "surface": "node-snapshot",
            "nodeId": node.id,
            "kind": node.kind,
            "kindVersion": node.kind_version,
            "portId": port_id,
        }),
    )
}

fn plan_edge_current(
    graph: &GraphDocumentCurrent,
    edge: &EdgeSpecCurrent,
    analysis: &GraphValidationResultCurrent,
) -> PlanEdge {
    let source = find_port(graph, &edge.source.node_id, &edge.source.port_id)
        .expect("current 0.1 validation should resolve source port");
    let target = find_port(graph, &edge.target.node_id, &edge.target.port_id)
        .expect("current 0.1 validation should resolve target port");

    PlanEdge {
        from_node: edge.source.node_id.clone(),
        from_port: edge.source.port_id.clone(),
        to_node: edge.target.node_id.clone(),
        to_port: edge.target.port_id.clone(),
        metadata: Some(PlanEdgeMetadata {
            resolved_type: Some(
                edge.resolved_type
                    .clone()
                    .unwrap_or_else(|| source.port_type.clone()),
            ),
            merge_policy: Some(merge_policy_label(target.merge_policy.as_ref())),
            fan_out_policy: Some(fan_out_policy_label(source.fan_out_policy.as_ref())),
            order: edge.order,
            feedback: edge.feedback.clone(),
            cycle_classification: cycle_classification_for_edge(edge, analysis),
        }),
    }
}

fn find_port<'a>(
    graph: &'a GraphDocumentCurrent,
    node_id: &str,
    port_id: &str,
) -> Option<&'a PortSpecCurrent> {
    graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)?
        .ports
        .iter()
        .find(|port| port.id == port_id)
}

fn cycle_classification_for_edge(
    edge: &EdgeSpecCurrent,
    analysis: &GraphValidationResultCurrent,
) -> Option<String> {
    analysis
        .cycles
        .iter()
        .find(|cycle| cycle.edges.iter().any(|edge_id| edge_id == &edge.id))
        .map(|cycle| cycle_validation_label(&cycle.classification).to_owned())
}

fn cycle_validation_label(classification: &CycleValidationCurrent) -> &'static str {
    match classification {
        CycleValidationCurrent::NoCycle => "no-cycle",
        CycleValidationCurrent::ValidFeedback => "valid-feedback",
        CycleValidationCurrent::RiskyFeedback => "risky-feedback",
        CycleValidationCurrent::AmbiguousAlgebraicLoop => "ambiguous-algebraic-loop",
        CycleValidationCurrent::InvalidCycle => "invalid-cycle",
    }
}

fn merge_policy_label(policy: Option<&MergePolicyCurrent>) -> String {
    match policy {
        Some(MergePolicyCurrent::OrderedEvents) => "ordered-events",
        Some(MergePolicyCurrent::Mix) => "mix",
        Some(MergePolicyCurrent::Array) => "array",
        Some(MergePolicyCurrent::Latest) => "latest",
        Some(MergePolicyCurrent::First) => "first",
        Some(MergePolicyCurrent::Custom) => "custom",
        Some(MergePolicyCurrent::Forbid) | None => "forbid",
    }
    .to_owned()
}

fn fan_out_policy_label(policy: Option<&FanOutPolicyCurrent>) -> String {
    match policy {
        Some(FanOutPolicyCurrent::Forbid) => "forbid",
        Some(FanOutPolicyCurrent::Copy) => "copy",
        Some(FanOutPolicyCurrent::Share) => "share",
        Some(FanOutPolicyCurrent::Allow) | None => "allow",
    }
    .to_owned()
}

fn map_execution_model_current(model: &ExecutionModelCurrent) -> ExecutionModel {
    match model {
        ExecutionModelCurrent::Event => ExecutionModel::Event,
        ExecutionModelCurrent::Control => ExecutionModel::Control,
        ExecutionModelCurrent::Frame => ExecutionModel::Frame,
        ExecutionModelCurrent::AudioBlock => ExecutionModel::AudioBlock,
        ExecutionModelCurrent::VideoFrame => ExecutionModel::VideoFrame,
        ExecutionModelCurrent::GpuPass => ExecutionModel::GpuPass,
        ExecutionModelCurrent::AsyncResource => ExecutionModel::AsyncResource,
        ExecutionModelCurrent::ScriptControl => ExecutionModel::ScriptControl,
        ExecutionModelCurrent::NativePlugin => ExecutionModel::NativePlugin,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        DiagnosticSeverity, FanOutPolicyCurrent, FeedbackBoundaryCurrent, PortDirectionCurrent,
        PortRateCurrent, PortSpecCurrent,
    };

    fn graph(value: Value) -> GraphDocumentCurrent {
        serde_json::from_value(value).expect("graph should parse")
    }

    fn definition(value: Value) -> NodeDefinitionCurrent {
        serde_json::from_value(value).expect("definition should parse")
    }

    fn clear_definition() -> NodeDefinitionCurrent {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.render.clear-color",
          "version": "0.1.0",
          "displayName": "Clear Color",
          "category": "Render",
          "ports": [
            { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn output_definition() -> NodeDefinitionCurrent {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.render.output",
          "version": "0.1.0",
          "displayName": "Render Output",
          "category": "Render",
          "ports": [
            { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn pass_definition() -> NodeDefinitionCurrent {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "test.pass",
          "version": "0.1.0",
          "displayName": "Pass",
          "category": "Test",
          "ports": [
            { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true },
            { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn behavior_definition(id: &str) -> NodeDefinitionCurrent {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": id,
          "version": "0.1.0",
          "displayName": id,
          "category": "Core",
          "ports": [],
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn render_graph() -> GraphDocumentCurrent {
        graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "render",
          "revision": "1",
          "nodes": [
            {
              "id": "clear",
              "kind": "object.core.render.clear-color",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
              ]
            },
            {
              "id": "output",
              "kind": "object.core.render.output",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true }
              ]
            }
          ],
          "edges": [
            {
              "id": "edge_clear_output",
              "source": { "nodeId": "clear", "portId": "out" },
              "target": { "nodeId": "output", "portId": "in" },
              "resolvedType": "value.core.tensor"
            }
          ]
        }))
    }

    fn identity_patch() -> PatchDefinitionCurrent {
        serde_json::from_value(json!({
          "id": "identity",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "identity-graph",
            "revision": "1",
            "nodes": [
              {
                "id": "patch_in",
                "kind": "object.core.inlet",
                "kindVersion": "0.1.0",
                "params": { "portId": "in", "label": "Input" },
                "ports": [
                  { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render", "description": "Frame entering the patch" }
                ]
              },
              {
                "id": "pass",
                "kind": "test.pass",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": [
                  { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true },
                  { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
                ]
              },
              {
                "id": "patch_out",
                "kind": "object.core.outlet",
                "kindVersion": "0.1.0",
                "params": { "portId": "out", "label": "Output" },
                "ports": [
                  { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true, "description": "Frame leaving the patch" }
                ]
              }
            ],
            "edges": [
              {
                "id": "edge_in_pass",
                "source": { "nodeId": "patch_in", "portId": "out" },
                "target": { "nodeId": "pass", "portId": "in" },
                "resolvedType": "value.core.tensor"
              },
              {
                "id": "edge_pass_out",
                "source": { "nodeId": "pass", "portId": "out" },
                "target": { "nodeId": "patch_out", "portId": "in" },
                "resolvedType": "value.core.tensor"
              }
            ]
          }
        }))
        .expect("patch definition should parse")
    }

    fn subpatch_graph() -> GraphDocumentCurrent {
        graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "render-subpatch",
          "revision": "1",
          "nodes": [
            {
              "id": "clear",
              "kind": "object.core.render.clear-color",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
              ]
            },
            {
              "id": "fx",
              "kind": "object.core.subpatch",
              "kindVersion": "0.1.0",
              "params": { "patchRef": "identity" },
              "ports": [
                { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true },
                { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
              ]
            },
            {
              "id": "output",
              "kind": "object.core.render.output",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render", "required": true }
              ]
            }
          ],
          "edges": [
            {
              "id": "edge_clear_fx",
              "source": { "nodeId": "clear", "portId": "out" },
              "target": { "nodeId": "fx", "portId": "in" },
              "resolvedType": "value.core.tensor"
            },
            {
              "id": "edge_fx_output",
              "source": { "nodeId": "fx", "portId": "out" },
              "target": { "nodeId": "output", "portId": "in" },
              "resolvedType": "value.core.tensor"
            }
          ]
        }))
    }

    fn project_document() -> ProjectDocumentCurrent {
        serde_json::from_value(json!({
          "schema": "skenion.project",
          "schemaVersion": "0.1.0",
          "id": "render-project",
          "revision": "1",
          "graph": subpatch_graph(),
          "viewState": {
            "schema": "skenion.view-state",
            "schemaVersion": "0.1.0",
            "canvas": { "nodes": {} }
          },
          "patchLibrary": [identity_patch()]
        }))
        .expect("project document should parse")
    }

    #[test]
    fn validates_and_builds_current_plan_metadata() {
        let graph = render_graph();
        let nodes = vec![clear_definition(), output_definition()];
        let (warnings, analysis) =
            validate_project_current(&graph, &nodes).expect("project should validate");
        assert!(warnings.is_empty());
        assert!(analysis.cycles.is_empty());

        let (plan, diagnostics) =
            build_execution_plan_current(&graph, &nodes).expect("plan should build");
        assert!(diagnostics.is_empty());
        assert_eq!(plan.graph_id, "render");
        assert_eq!(plan.nodes.len(), 2);
        assert_eq!(plan.groups.len(), 1);
        let metadata = plan.edges[0]
            .metadata
            .as_ref()
            .expect("metadata should exist");
        assert_eq!(metadata.resolved_type.as_deref(), Some("value.core.tensor"));
        assert_eq!(metadata.merge_policy.as_deref(), Some("forbid"));
        assert_eq!(metadata.fan_out_policy.as_deref(), Some("allow"));
        assert_eq!(metadata.cycle_classification, None);
    }

    #[test]
    fn records_merge_order_feedback_and_risky_warnings() {
        let mut graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "feedback",
          "revision": "1",
          "nodes": [
            {
              "id": "node",
              "kind": "object.core.render.feedback-composite",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "previous", "direction": "input", "type": "value.core.tensor", "rate": "render" },
                { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render", "fanOutPolicy": "copy" }
              ]
            }
          ],
          "edges": [
            {
              "id": "edge_feedback",
              "source": { "nodeId": "node", "portId": "out" },
              "target": { "nodeId": "node", "portId": "previous" },
              "order": 3,
              "feedback": { "enabled": true, "boundary": "render-frame", "intentional": true }
            }
          ]
        }));
        let definition = definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.render.feedback-composite",
          "version": "0.1.0",
          "displayName": "Feedback",
          "category": "Render",
          "ports": [
            { "id": "previous", "direction": "input", "type": "value.core.tensor", "rate": "render" },
            { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render", "fanOutPolicy": "copy" }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }));

        let (plan, diagnostics) =
            build_execution_plan_current(&graph, std::slice::from_ref(&definition))
                .expect("feedback should plan");
        assert!(diagnostics.is_empty());
        let metadata = plan.edges[0].metadata.as_ref().unwrap();
        assert_eq!(metadata.order, Some(3));
        assert_eq!(metadata.fan_out_policy.as_deref(), Some("copy"));
        assert_eq!(
            metadata.cycle_classification.as_deref(),
            Some("valid-feedback")
        );
        assert_eq!(
            metadata.feedback.as_ref().unwrap().boundary,
            FeedbackBoundaryCurrent::RenderFrame
        );

        graph.edges[0].feedback.as_mut().unwrap().boundary = FeedbackBoundaryCurrent::SameTurn;
        let (_plan, diagnostics) = build_execution_plan_current(&graph, &[definition])
            .expect("risky feedback should plan");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == DiagnosticSeverity::Warning
                && diagnostic.code.as_deref() == Some("graph.risky-feedback")
        }));
    }

    #[test]
    fn project_document_conversions_default_runtime_fields() {
        let document = project_document();

        let request: ProjectRequestCurrent = document.clone().into();
        assert_eq!(request.graph.id, "render-subpatch");
        assert!(request.nodes.is_empty());
        assert_eq!(request.patch_library[0].id, "identity");

        let run_request: RunProjectRequestCurrent = document.into();
        assert_eq!(run_request.graph.id, "render-subpatch");
        assert!(run_request.nodes.is_empty());
        assert_eq!(run_request.patch_library[0].id, "identity");
        assert_eq!(run_request.frames, None);
    }

    #[test]
    fn expands_subpatches_before_current_validation_and_planning() {
        let request = ProjectRequestCurrent {
            document: None,
            graph: subpatch_graph(),
            nodes: vec![clear_definition(), output_definition(), pass_definition()],
            patch_library: vec![identity_patch()],
            view_state: None,
        };

        let expanded = expand_project_graph_current(&request.graph, &request.patch_library)
            .expect("subpatch graph should expand");
        let node_ids = expanded
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(node_ids, vec!["clear", "fx::pass", "output"]);
        assert!(expanded.edges.iter().any(|edge| {
            edge.source.node_id == "clear"
                && edge.source.port_id == "out"
                && edge.target.node_id == "fx::pass"
                && edge.target.port_id == "in"
        }));
        assert!(expanded.edges.iter().any(|edge| {
            edge.source.node_id == "fx::pass"
                && edge.source.port_id == "out"
                && edge.target.node_id == "output"
                && edge.target.port_id == "in"
        }));

        let (diagnostics, _) =
            validate_project_request_current(&request).expect("expanded project should validate");
        assert!(diagnostics.is_empty());
        let (plan, diagnostics) =
            build_execution_plan_request_current(&request).expect("expanded project should plan");
        assert!(diagnostics.is_empty());
        assert_eq!(
            plan.nodes
                .iter()
                .map(|node| node.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["clear", "fx::pass", "output"]
        );
    }

    #[test]
    fn current_plan_sorts_expanded_nodes_by_dependency_order() {
        let mut graph = subpatch_graph();
        graph.nodes.reverse();
        let request = ProjectRequestCurrent {
            document: None,
            graph,
            nodes: vec![clear_definition(), output_definition(), pass_definition()],
            patch_library: vec![identity_patch()],
            view_state: None,
        };

        let expanded = expand_project_graph_current(&request.graph, &request.patch_library)
            .expect("subpatch graph should expand");
        assert_eq!(
            expanded
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["output", "fx::pass", "clear"]
        );

        let (plan, diagnostics) =
            build_execution_plan_request_current(&request).expect("expanded project should plan");

        assert!(diagnostics.is_empty());
        assert_eq!(
            plan.nodes
                .iter()
                .map(|node| node.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["clear", "fx::pass", "output"]
        );
        assert_eq!(
            plan.nodes.iter().map(|node| node.order).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn contracts_boundary_edges_and_filters_boundary_only_edges() {
        let base_edge = render_graph().edges[0].clone();
        let endpoint = EdgeEndpointCurrent {
            node_id: "same".to_owned(),
            port_id: "out".to_owned(),
        };
        let self_loop_edges = vec![
            ExpansionEdge {
                edge: base_edge.clone(),
                source: ExpansionEndpoint::Node(endpoint.clone()),
                target: ExpansionEndpoint::Boundary("pin".to_owned()),
            },
            ExpansionEdge {
                edge: base_edge.clone(),
                source: ExpansionEndpoint::Boundary("pin".to_owned()),
                target: ExpansionEndpoint::Node(endpoint.clone()),
            },
        ];

        let contracted = contract_boundary_edges(
            self_loop_edges,
            std::collections::HashSet::from(["pin".to_owned()]),
        );
        assert!(contracted.is_empty());

        let mut source_edge = base_edge.clone();
        source_edge.id = "source_edge".to_owned();
        source_edge.resolved_type = Some("value.core.tensor".to_owned());
        let mut target_edge = base_edge.clone();
        target_edge.id = "target_edge".to_owned();
        target_edge.resolved_type = None;
        let merged = contract_boundary_edges(
            vec![
                ExpansionEdge {
                    edge: source_edge,
                    source: ExpansionEndpoint::Node(EdgeEndpointCurrent {
                        node_id: "source".to_owned(),
                        port_id: "out".to_owned(),
                    }),
                    target: ExpansionEndpoint::Boundary("fx::@inlet::in".to_owned()),
                },
                ExpansionEdge {
                    edge: target_edge,
                    source: ExpansionEndpoint::Boundary("fx::@inlet::in".to_owned()),
                    target: ExpansionEndpoint::Node(EdgeEndpointCurrent {
                        node_id: "target".to_owned(),
                        port_id: "in".to_owned(),
                    }),
                },
            ],
            std::collections::HashSet::from(["fx::@inlet::in".to_owned()]),
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].resolved_type.as_deref(),
            Some("value.core.tensor")
        );
        assert!(merged[0].id.contains("fx___inlet__in"));

        assert!(
            expansion_edge_to_real_edge(ExpansionEdge {
                edge: base_edge.clone(),
                source: ExpansionEndpoint::Boundary("pin".to_owned()),
                target: ExpansionEndpoint::Node(endpoint.clone()),
            })
            .is_none()
        );
        assert!(
            expansion_edge_to_real_edge(ExpansionEdge {
                edge: base_edge,
                source: ExpansionEndpoint::Node(endpoint),
                target: ExpansionEndpoint::Boundary("pin".to_owned()),
            })
            .is_none()
        );
    }

    #[test]
    fn reports_missing_ref_depth_and_duplicate_patch_diagnostics() {
        let duplicate = ProjectRequestCurrent {
            document: None,
            graph: render_graph(),
            nodes: vec![clear_definition(), output_definition()],
            patch_library: vec![identity_patch(), identity_patch()],
            view_state: None,
        };
        let duplicate_diagnostics = validate_project_request_current(&duplicate)
            .expect_err("duplicate patch ids should fail");
        assert_eq!(
            duplicate_diagnostics[0].code.as_deref(),
            Some("subpatch.duplicate-patch-id")
        );

        let missing_ref = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "missing-ref",
          "revision": "1",
          "nodes": [
            {
              "id": "fx",
              "kind": "object.core.subpatch",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": []
            }
          ],
          "edges": []
        }));
        let missing_ref_diagnostics =
            expand_project_graph_current(&missing_ref, &[]).expect_err("missing ref should fail");
        assert_eq!(
            missing_ref_diagnostics[0].code.as_deref(),
            Some("subpatch.missing-ref")
        );
        assert_eq!(
            missing_ref_diagnostics[0].details.as_ref().unwrap()["patchRef"],
            Value::Null
        );

        let mut patch_library = Vec::new();
        for index in 0..=15 {
            patch_library.push(
                serde_json::from_value(json!({
                  "id": format!("p{index}"),
                  "revision": "1",
                  "graph": {
                    "schema": "skenion.graph",
                    "schemaVersion": "0.1.0",
                    "id": format!("p{index}-graph"),
                    "revision": "1",
                    "nodes": [
                      {
                        "id": "next",
                        "kind": "object.core.subpatch",
                        "kindVersion": "0.1.0",
                        "params": { "patchRef": format!("p{}", index + 1) },
                        "ports": []
                      }
                    ],
                    "edges": []
                  }
                }))
                .expect("patch should parse"),
            );
        }
        let depth_root = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "depth-root",
          "revision": "1",
          "nodes": [
            {
              "id": "root",
              "kind": "object.core.subpatch",
              "kindVersion": "0.1.0",
              "params": { "patchRef": "p0" },
              "ports": []
            }
          ],
          "edges": []
        }));
        let depth_diagnostics = expand_project_graph_current(&depth_root, &patch_library)
            .expect_err("depth should fail");
        assert_eq!(
            depth_diagnostics[0].code.as_deref(),
            Some("subpatch.depth-exceeded")
        );
        assert_eq!(depth_diagnostics[0].details.as_ref().unwrap()["depth"], 17);
    }

    #[test]
    fn parses_subpatch_aliases_and_reports_missing_boundaries() {
        assert_eq!(
            parse_subpatch_object_text("p identity").as_deref(),
            Some("identity")
        );
        assert_eq!(
            parse_subpatch_object_text("object.core.subpatch identity").as_deref(),
            Some("identity")
        );
        assert_eq!(parse_subpatch_object_text("object identity"), None);
        assert_eq!(namespace_prefix(""), "");

        let params = serde_json::Map::from_iter([
            ("patchId".to_owned(), json!(42)),
            ("empty".to_owned(), json!("")),
            ("enabled".to_owned(), json!(true)),
        ]);
        assert_eq!(string_param(&params, "patchId").as_deref(), Some("42"));
        assert_eq!(string_param(&params, "empty"), None);
        assert_eq!(string_param(&params, "enabled"), None);

        let fallback_boundary = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "fallback-boundary",
          "revision": "1",
          "nodes": [
            {
              "id": "plain_inlet",
              "kind": "object.core.inlet",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": []
            }
          ],
          "edges": []
        }));
        assert_eq!(boundary_key(&fallback_boundary.nodes[0]), "plain_inlet");

        let duplicate_inlet_patch: PatchDefinitionCurrent = serde_json::from_value(json!({
          "id": "alias-patch",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "alias-patch-graph",
            "revision": "1",
            "nodes": [
              {
                "id": "in_a",
                "kind": "object.core.inlet",
                "kindVersion": "0.1.0",
                "params": { "portId": "in_a", "label": "shared" },
                "ports": [
                  { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
                ]
              },
              {
                "id": "in_b",
                "kind": "object.core.inlet",
                "kindVersion": "0.1.0",
                "params": { "portId": "in_b", "label": "shared" },
                "ports": [
                  { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
                ]
              }
            ],
            "edges": []
          }
        }))
        .expect("patch should parse");
        let mut boundary_pins = std::collections::HashSet::new();
        let mut aliases = std::collections::HashMap::new();
        let first_pin = register_boundary_node(
            &duplicate_inlet_patch.graph.nodes[0],
            "fx",
            BoundaryKind::Inlet,
            &mut boundary_pins,
            &mut aliases,
        );
        let second_pin = register_boundary_node(
            &duplicate_inlet_patch.graph.nodes[0],
            "fx",
            BoundaryKind::Inlet,
            &mut boundary_pins,
            &mut aliases,
        );
        assert_eq!(first_pin, second_pin);

        let root = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "alias-root",
          "revision": "1",
          "nodes": [
            {
              "id": "clear",
              "kind": "object.core.render.clear-color",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
              ]
            },
            {
              "id": "fx",
              "kind": "p",
              "kindVersion": "0.1.0",
              "params": { "objectText": "p alias-patch" },
              "ports": [
                { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render" },
                { "id": "out", "direction": "output", "type": "value.core.tensor", "rate": "render" }
              ]
            },
            {
              "id": "output",
              "kind": "object.core.render.output",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": "value.core.tensor", "rate": "render" }
              ]
            }
          ],
          "edges": [
            {
              "id": "edge_clear_fx",
              "source": { "nodeId": "clear", "portId": "out" },
              "target": { "nodeId": "fx", "portId": "shared" }
            },
            {
              "id": "edge_fx_output",
              "source": { "nodeId": "fx", "portId": "out" },
              "target": { "nodeId": "output", "portId": "in" }
            }
          ]
        }));
        let diagnostics = expand_project_graph_current(&root, &[duplicate_inlet_patch])
            .expect_err("boundaries fail");
        let codes = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_deref())
            .collect::<Vec<_>>();
        assert!(codes.contains(&Some("subpatch.missing-inlet")));
        assert!(codes.contains(&Some("subpatch.missing-outlet")));
    }

    #[test]
    fn reports_missing_recursive_and_invalid_patch_library_diagnostics() {
        let missing = ProjectRequestCurrent {
            document: None,
            graph: subpatch_graph(),
            nodes: vec![clear_definition(), output_definition(), pass_definition()],
            patch_library: Vec::new(),
            view_state: None,
        };
        let missing_diagnostics =
            validate_project_request_current(&missing).expect_err("missing patch should fail");
        assert_eq!(
            missing_diagnostics[0].code.as_deref(),
            Some("subpatch.missing-patch")
        );

        let recursive_patch: PatchDefinitionCurrent = serde_json::from_value(json!({
          "id": "recursive",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "recursive-graph",
            "revision": "1",
            "nodes": [
              {
                "id": "self",
                "kind": "object.core.subpatch",
                "kindVersion": "0.1.0",
                "params": { "patchRef": "recursive" },
                "ports": []
              }
            ],
            "edges": []
          }
        }))
        .expect("recursive patch should parse");
        let recursive = ProjectRequestCurrent {
            document: None,
            graph: graph(json!({
              "schema": "skenion.graph",
              "schemaVersion": "0.1.0",
              "id": "recursive-root",
              "revision": "1",
              "nodes": [
                {
                  "id": "root",
                  "kind": "object.core.subpatch",
                  "kindVersion": "0.1.0",
                  "params": { "patchRef": "recursive" },
                  "ports": []
                }
              ],
              "edges": []
            })),
            nodes: Vec::new(),
            patch_library: vec![recursive_patch],
            view_state: None,
        };
        let recursive_diagnostics =
            validate_project_request_current(&recursive).expect_err("recursive patch should fail");
        assert_eq!(
            recursive_diagnostics[0].code.as_deref(),
            Some("subpatch.recursion")
        );

        let mut duplicate_boundary = identity_patch();
        duplicate_boundary.graph.nodes[2].params["portId"] = json!("in");
        let invalid = ProjectRequestCurrent {
            document: None,
            graph: render_graph(),
            nodes: vec![clear_definition(), output_definition()],
            patch_library: vec![duplicate_boundary],
            view_state: None,
        };
        let invalid_diagnostics =
            validate_project_request_current(&invalid).expect_err("invalid patch should fail");
        assert_eq!(
            invalid_diagnostics[0].code.as_deref(),
            Some("subpatch.invalid-patch-definition")
        );
    }

    fn assert_patch_graph_schema_diagnostic(
        diagnostics: &[RuntimeDiagnostic],
        expected_code: &str,
        expected_received_schema_version: &str,
    ) {
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code.as_deref() == Some(expected_code))
            .unwrap_or_else(|| panic!("missing {expected_code} diagnostic: {diagnostics:#?}"));
        let details = diagnostic
            .details
            .as_ref()
            .expect("schema diagnostic should include details");

        assert_eq!(details["surface"], "graph");
        assert_eq!(details["patchId"], "identity");
        assert_eq!(details["expectedSchemaVersion"], "0.1.0");
        assert_eq!(
            details["receivedSchemaVersion"],
            expected_received_schema_version
        );
        assert!(
            diagnostics.iter().all(|diagnostic| {
                diagnostic.code.as_deref() != Some("subpatch.invalid-patch-definition")
                    || !diagnostic.message.contains("schemaVersion")
            }),
            "patch graph schemaVersion should not also be reported as generic patch contract failure: {diagnostics:#?}"
        );
    }

    #[test]
    fn direct_requests_report_structured_patch_graph_schema_versions() {
        let schema_version = "9.9.9";
        let expected_code = "project.unsupported-schema-version";
        let mut patch = identity_patch();
        patch.graph.schema_version = schema_version.to_owned();
        let request = ProjectRequestCurrent {
            document: None,
            graph: render_graph(),
            nodes: vec![clear_definition(), output_definition(), pass_definition()],
            patch_library: vec![patch],
            view_state: None,
        };

        let validation_diagnostics = validate_project_request_current(&request)
            .expect_err("patch graph schema mismatch should fail request validation");
        assert_patch_graph_schema_diagnostic(
            &validation_diagnostics,
            expected_code,
            schema_version,
        );

        let planning_diagnostics = build_execution_plan_request_current(&request)
            .expect_err("patch graph schema mismatch should fail request planning");
        assert_patch_graph_schema_diagnostic(&planning_diagnostics, expected_code, schema_version);
    }

    #[test]
    fn rejects_payload_identity_node_kinds_and_definition_ids() {
        for payload_identity in [
            "object.core.bool",
            "object.core.string",
            "bool",
            "string",
            "value.number",
            "value.core.message",
            "value.core.bang",
            "value.core.string",
            "value.core.tensor",
        ] {
            let mut graph = render_graph();
            graph.nodes[0].kind = payload_identity.to_owned();
            graph.nodes[0].ports.clear();
            let graph_result =
                validate_project_current(&graph, &[clear_definition(), output_definition()])
                    .expect_err("payload identity graph node kind should fail");
            assert!(
                graph_result.iter().any(|diagnostic| {
                    diagnostic.code.as_deref() == Some("graph.payload-node-kind")
                        && diagnostic.details.as_ref().unwrap()["kind"] == payload_identity
                }),
                "{payload_identity}: {graph_result:#?}"
            );

            let mut definition = clear_definition();
            definition.id = payload_identity.to_owned();
            let definition_result =
                validate_project_current(&render_graph(), &[definition, output_definition()])
                    .expect_err("payload identity definition id should fail");
            assert!(
                definition_result.iter().any(|diagnostic| {
                    diagnostic.code.as_deref() == Some("node-definition.payload-identity-id")
                        && diagnostic.details.as_ref().unwrap()["nodeDefinitionId"]
                            == payload_identity
                }),
                "{payload_identity}: {definition_result:#?}"
            );
        }
    }

    #[test]
    fn accepts_behavior_object_identities_that_still_exist() {
        let behavior_ids = [
            "object.core.float",
            "object.core.int",
            "object.core.uint",
            "object.core.bang",
            "object.core.message",
        ];
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "behavior-identities",
          "revision": "1",
          "nodes": behavior_ids
            .iter()
            .enumerate()
            .map(|(index, kind)| json!({
              "id": format!("node_{index}"),
              "kind": kind,
              "kindVersion": "0.1.0",
              "params": {},
              "ports": []
            }))
            .collect::<Vec<_>>(),
          "edges": []
        }));
        let definitions = behavior_ids
            .iter()
            .map(|id| {
                definition(json!({
                  "schema": "skenion.node.definition",
                  "schemaVersion": "0.1.0",
                  "id": id,
                  "version": "0.1.0",
                  "displayName": id,
                  "category": "Core",
                  "ports": [],
                  "execution": { "model": "control" },
                  "state": { "persistent": false },
                  "permissions": [],
                  "capabilities": []
                }))
            })
            .collect::<Vec<_>>();

        let (diagnostics, _) =
            validate_project_current(&graph, &definitions).expect("behavior ids should validate");

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn validates_runtime_owned_object_text_resolution() {
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "object-text",
          "revision": "1",
          "nodes": [
            {
              "id": "add",
              "kind": "object.core.operator.add",
              "kindVersion": "0.1.0",
              "objectText": "+ 2",
              "params": {},
              "ports": []
            }
          ],
          "edges": []
        }));

        validate_project_current(&graph, &[behavior_definition("object.core.operator.add")])
            .expect("matching Runtime object text should validate");

        let mut invalid_arg = graph.clone();
        invalid_arg.nodes[0].object_text = Some("+ true".to_owned());
        let invalid_arg_result = validate_project_current(
            &invalid_arg,
            &[behavior_definition("object.core.operator.add")],
        )
        .expect_err("invalid Runtime object-text args should fail");
        assert!(invalid_arg_result.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("object-text.invalid-arg-type")
                && diagnostic.details.as_ref().unwrap()["objectText"] == "+ true"
        }));

        let mut mismatch = graph.clone();
        mismatch.nodes[0].kind = "object.core.operator.sub".to_owned();
        let mismatch_result = validate_project_current(
            &mismatch,
            &[behavior_definition("object.core.operator.sub")],
        )
        .expect_err("resolved object kind mismatch should fail");
        assert!(mismatch_result.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("object-text.kind-mismatch")
                && diagnostic.details.as_ref().unwrap()["resolvedKind"]
                    == "object.core.operator.add"
                && diagnostic.details.as_ref().unwrap()["nodeKind"] == "object.core.operator.sub"
        }));

        let mut payload = graph.clone();
        payload.nodes[0].kind = "object.core.float".to_owned();
        payload.nodes[0].object_text = Some("value.core.float32".to_owned());
        let payload_result =
            validate_project_current(&payload, &[behavior_definition("object.core.float")])
                .expect_err("payload identity object text should fail");
        assert!(payload_result.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("object-text.payload-identity")
                && diagnostic.details.as_ref().unwrap()["objectText"] == "value.core.float32"
        }));

        let mut package_deferred = graph.clone();
        package_deferred.nodes[0].kind = "user.manipulator".to_owned();
        package_deferred.nodes[0].object_text = Some("user.manipulator 1".to_owned());
        validate_project_current(
            &package_deferred,
            &[behavior_definition("user.manipulator")],
        )
        .expect("package-owned object text remains available to package resolver layers");
    }

    #[test]
    fn surfaces_selector_and_connection_policy_diagnostics_with_specific_codes() {
        let mut selector_graph = render_graph();
        selector_graph.nodes[1].ports[0].port_type = "value.core.message".to_owned();
        selector_graph.nodes[1].ports[0].rate = Some(PortRateCurrent::Control);
        selector_graph.nodes[1].ports[0].message_keys = None;
        let mut selector_output_definition = output_definition();
        selector_output_definition.ports[0] = selector_graph.nodes[1].ports[0].clone();
        let selector_result = validate_project_current(
            &selector_graph,
            &[clear_definition(), selector_output_definition],
        )
        .expect_err("selector-aware input port should fail without selector policy");
        assert!(
            selector_result.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("graph.message-key-policy")
                    && diagnostic
                        .message
                        .contains("message-key-aware input port requires messageKeys")
            }),
            "{selector_result:#?}"
        );

        let mut fan_in_graph = render_graph();
        let mut clear_two = fan_in_graph.nodes[0].clone();
        clear_two.id = "clear_two".to_owned();
        fan_in_graph.nodes.push(clear_two);
        fan_in_graph.edges.push(EdgeSpecCurrent {
            id: "edge_clear_two_output".to_owned(),
            source: EdgeEndpointCurrent {
                node_id: "clear_two".to_owned(),
                port_id: "out".to_owned(),
            },
            target: EdgeEndpointCurrent {
                node_id: "output".to_owned(),
                port_id: "in".to_owned(),
            },
            resolved_type: Some("value.core.tensor".to_owned()),
            order: None,
            enabled: None,
            adapter: None,
            feedback: None,
            style_override: None,
            label: None,
            description: None,
        });
        let fan_in_result =
            validate_project_current(&fan_in_graph, &[clear_definition(), output_definition()])
                .expect_err("default input fan-in should fail");
        assert!(
            fan_in_result
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("graph.fan-in-cardinality")),
            "{fan_in_result:#?}"
        );

        let mut fan_out_graph = render_graph();
        fan_out_graph.nodes[0].ports[0].fan_out_policy = Some(FanOutPolicyCurrent::Forbid);
        let mut output_two = fan_out_graph.nodes[1].clone();
        output_two.id = "output_two".to_owned();
        fan_out_graph.nodes.push(output_two);
        fan_out_graph.edges.push(EdgeSpecCurrent {
            id: "edge_clear_output_two".to_owned(),
            source: EdgeEndpointCurrent {
                node_id: "clear".to_owned(),
                port_id: "out".to_owned(),
            },
            target: EdgeEndpointCurrent {
                node_id: "output_two".to_owned(),
                port_id: "in".to_owned(),
            },
            resolved_type: Some("value.core.tensor".to_owned()),
            order: None,
            enabled: None,
            adapter: None,
            feedback: None,
            style_override: None,
            label: None,
            description: None,
        });
        let mut fan_out_clear_definition = clear_definition();
        fan_out_clear_definition.ports[0].fan_out_policy = Some(FanOutPolicyCurrent::Forbid);
        let fan_out_result = validate_project_current(
            &fan_out_graph,
            &[fan_out_clear_definition, output_definition()],
        )
        .expect_err("forbidden output fan-out should fail");
        assert!(
            fan_out_result
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("graph.fan-out-forbidden")),
            "{fan_out_result:#?}"
        );
    }

    #[test]
    fn rejects_invalid_graph_definitions_and_snapshots() {
        let graph = render_graph();
        let missing = validate_project_current(&graph, &[]).expect_err("missing definitions fail");
        assert_eq!(missing[0].code.as_deref(), Some("node-definition.missing"));
        assert_eq!(
            missing[0].details.as_ref().unwrap()["surface"],
            "node-definition"
        );

        let mut unsupported_graph = render_graph();
        unsupported_graph.schema_version = "9.9.9".to_owned();
        let unsupported_graph_result = validate_project_current(
            &unsupported_graph,
            &[clear_definition(), output_definition()],
        )
        .expect_err("unsupported graph schema should fail");
        assert_eq!(
            unsupported_graph_result[0].code.as_deref(),
            Some("graph.invalid-contract")
        );
        assert_eq!(
            unsupported_graph_result[0].details.as_ref().unwrap()["surface"],
            "graph"
        );
        assert_eq!(
            unsupported_graph_result[0].details.as_ref().unwrap()["expectedSchemaVersion"],
            "0.1.0"
        );
        assert_eq!(
            unsupported_graph_result[0].details.as_ref().unwrap()["receivedSchemaVersion"],
            "9.9.9"
        );

        let mut invalid_definition = clear_definition();
        invalid_definition.permissions.push("network".to_owned());
        let invalid_definition_result =
            validate_project_current(&graph, &[invalid_definition, output_definition()])
                .expect_err("invalid definition should fail");
        let invalid_definition_diagnostic = invalid_definition_result
            .iter()
            .find(|diagnostic| {
                diagnostic
                    .message
                    .contains("unsupported permission: network")
            })
            .expect("unsupported permission should be reported");
        assert_eq!(
            invalid_definition_diagnostic.code.as_deref(),
            Some("node-definition.invalid-contract")
        );
        assert_eq!(
            invalid_definition_diagnostic.details.as_ref().unwrap()["surface"],
            "node-definition"
        );
        assert_eq!(
            invalid_definition_diagnostic.details.as_ref().unwrap()["expectedSchemaVersion"],
            "0.1.0"
        );
        assert_eq!(
            invalid_definition_diagnostic.details.as_ref().unwrap()["receivedSchemaVersion"],
            "0.1.0"
        );

        let mut mismatch = render_graph();
        mismatch.nodes[0].ports.clear();
        mismatch.nodes[1].ports[0].direction = PortDirectionCurrent::Output;
        mismatch.nodes[1].ports[0].port_type = "value.core.float32".to_owned();
        mismatch.nodes[1].ports.push(PortSpecCurrent {
            id: "extra".to_owned(),
            direction: PortDirectionCurrent::Input,
            port_type: "value.core.tensor".to_owned(),
            label: None,
            rate: None,
            accepts: None,
            min_connections: None,
            max_connections: None,
            merge_policy: None,
            fan_out_policy: None,
            trigger_mode: None,
            message_keys: None,
            default_value: None,
            latch: None,
            required: None,
            style_key: None,
            group: None,
            description: None,
        });
        let mismatch_result =
            validate_project_current(&mismatch, &[clear_definition(), output_definition()])
                .expect_err("snapshot mismatch should fail");
        let messages = mismatch_result
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            mismatch_result
                .iter()
                .all(|diagnostic| diagnostic.code.is_some()),
            "current 0.1 project diagnostics should be structured"
        );
        assert!(messages.contains("missing manifest port"));
        assert!(messages.contains("direction differs from definition"));
        assert!(messages.contains("type value.core.float32"));
        assert!(messages.contains("missing source port"));
        assert!(messages.contains("missing manifest port: output.extra"));

        let mut incompatible = render_graph();
        incompatible.nodes[1].ports[0].port_type = "value.core.message".to_owned();
        incompatible.nodes[1].ports[0].rate = Some(PortRateCurrent::Event);
        let incompatible_result =
            validate_project_current(&incompatible, &[clear_definition(), output_definition()])
                .expect_err("incompatible edge type should fail");
        assert!(incompatible_result.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("graph.edge-incompatible-type")
                && diagnostic.message.contains(
                    "incompatible edge clear:out value.core.tensor -> output:in value.core.message",
                )
        }));
    }

    #[test]
    fn labels_all_current_policy_and_execution_variants() {
        for (policy, expected) in [
            (Some(MergePolicyCurrent::Forbid), "forbid"),
            (Some(MergePolicyCurrent::OrderedEvents), "ordered-events"),
            (Some(MergePolicyCurrent::Mix), "mix"),
            (Some(MergePolicyCurrent::Array), "array"),
            (Some(MergePolicyCurrent::Latest), "latest"),
            (Some(MergePolicyCurrent::First), "first"),
            (Some(MergePolicyCurrent::Custom), "custom"),
            (None, "forbid"),
        ] {
            assert_eq!(merge_policy_label(policy.as_ref()), expected);
        }

        for (policy, expected) in [
            (Some(FanOutPolicyCurrent::Allow), "allow"),
            (Some(FanOutPolicyCurrent::Forbid), "forbid"),
            (Some(FanOutPolicyCurrent::Copy), "copy"),
            (Some(FanOutPolicyCurrent::Share), "share"),
            (None, "allow"),
        ] {
            assert_eq!(fan_out_policy_label(policy.as_ref()), expected);
        }

        for (classification, expected) in [
            (CycleValidationCurrent::NoCycle, "no-cycle"),
            (CycleValidationCurrent::ValidFeedback, "valid-feedback"),
            (CycleValidationCurrent::RiskyFeedback, "risky-feedback"),
            (
                CycleValidationCurrent::AmbiguousAlgebraicLoop,
                "ambiguous-algebraic-loop",
            ),
            (CycleValidationCurrent::InvalidCycle, "invalid-cycle"),
        ] {
            assert_eq!(cycle_validation_label(&classification), expected);
        }

        for (model, expected) in [
            (ExecutionModelCurrent::Event, ExecutionModel::Event),
            (ExecutionModelCurrent::Control, ExecutionModel::Control),
            (ExecutionModelCurrent::Frame, ExecutionModel::Frame),
            (
                ExecutionModelCurrent::AudioBlock,
                ExecutionModel::AudioBlock,
            ),
            (
                ExecutionModelCurrent::VideoFrame,
                ExecutionModel::VideoFrame,
            ),
            (ExecutionModelCurrent::GpuPass, ExecutionModel::GpuPass),
            (
                ExecutionModelCurrent::AsyncResource,
                ExecutionModel::AsyncResource,
            ),
            (
                ExecutionModelCurrent::ScriptControl,
                ExecutionModel::ScriptControl,
            ),
            (
                ExecutionModelCurrent::NativePlugin,
                ExecutionModel::NativePlugin,
            ),
        ] {
            assert_eq!(map_execution_model_current(&model), expected);
        }
    }
}
