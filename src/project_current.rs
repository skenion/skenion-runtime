use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::current_node_identity::{
    CURRENT_OBJECT_VERSION, graph_node_executable_kind, graph_node_executable_kind_version,
    graph_node_object_id,
};
use crate::object_spec::{
    ObjectRegistry, PROJECT_PATCH_OBJECT_KIND_PREFIX, is_payload_identity_kind,
    resolve_object_spec_v01,
};
use crate::{
    CycleValidationCurrent, EdgeEndpointCurrent, EdgeSpecCurrent, ExecutionGroup, ExecutionModel,
    ExecutionModelCurrent, FanOutPolicyCurrent, GraphDocumentCurrent, GraphNodeCurrent,
    GraphValidationResultCurrent, MergePolicyCurrent, NodeDefinitionCurrent,
    PatchDefinitionCurrent, PlanEdge, PlanEdgeMetadata, PlanNode, PortDirectionCurrent,
    PortSpecCurrent, ProjectDocumentCurrent, RuntimeDiagnostic, ViewState, port_connection_policy,
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
    let nodes = normalized_node_definitions_current(&request.nodes, &request.patch_library);
    validate_project_current(&graph, &nodes)
}

pub fn build_execution_plan_request_current(
    request: &ProjectRequestCurrent,
) -> Result<(crate::ExecutionPlan, Vec<RuntimeDiagnostic>), Vec<RuntimeDiagnostic>> {
    validate_patch_library_current(&request.patch_library)?;
    let graph = expand_project_graph_current(&request.graph, &request.patch_library)?;
    let nodes = normalized_node_definitions_current(&request.nodes, &request.patch_library);
    build_execution_plan_current(&graph, &nodes)
}

pub fn build_execution_plan_run_request_current(
    request: &RunProjectRequestCurrent,
) -> Result<(crate::ExecutionPlan, Vec<RuntimeDiagnostic>), Vec<RuntimeDiagnostic>> {
    validate_patch_library_current(&request.patch_library)?;
    let graph = expand_project_graph_current(&request.graph, &request.patch_library)?;
    let nodes = normalized_node_definitions_current(&request.nodes, &request.patch_library);
    build_execution_plan_current(&graph, &nodes)
}

fn normalized_node_definitions_current(
    explicit_nodes: &[NodeDefinitionCurrent],
    patch_library: &[PatchDefinitionCurrent],
) -> Vec<NodeDefinitionCurrent> {
    let mut nodes = explicit_nodes.to_vec();
    let mut seen = nodes
        .iter()
        .map(|definition| (definition.id.clone(), definition.version.clone()))
        .collect::<HashSet<_>>();

    for definition in ObjectRegistry::for_patch_library(patch_library).node_definition_projection()
    {
        let key = (definition.id.clone(), definition.version.clone());
        if seen.insert(key) {
            nodes.push(definition);
        }
    }

    nodes
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
            if graph_node_object_id(node).is_some_and(is_payload_identity_node_kind_current) {
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
        .or_else(|| subpatch_object_spec(node).and_then(|text| parse_subpatch_object_spec(&text)))
        .or_else(|| {
            graph_node_executable_kind(node)?
                .strip_prefix(PROJECT_PATCH_OBJECT_KIND_PREFIX)
                .map(ToOwned::to_owned)
        })
}

fn parse_subpatch_object_spec(text: &str) -> Option<String> {
    let resolution = resolve_object_spec_v01(text);
    if resolution
        .implementation
        .as_ref()
        .map(|implementation| {
            crate::current_node_identity::implementation_executable_kind(implementation)
        })
        .as_deref()
        != Some(SUBPATCH_KIND)
        || !resolution.ok()
    {
        return None;
    }
    resolution
        .params
        .get("patchRef")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn subpatch_object_spec(node: &GraphNodeCurrent) -> Option<String> {
    node.object_spec.clone()
}

fn node_object_spec(node: &GraphNodeCurrent) -> Option<String> {
    node.object_spec.clone()
}

fn string_param(params: &Map<String, Value>, key: &str) -> Option<String> {
    match params.get(key)? {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn is_subpatch_node(node: &GraphNodeCurrent) -> bool {
    graph_node_executable_kind(node).is_some_and(|kind| {
        matches!(kind.as_str(), SUBPATCH_KIND | SUBPATCH_SHORTHAND_KIND)
            || kind.starts_with(PROJECT_PATCH_OBJECT_KIND_PREFIX)
    })
}

fn is_inlet_node(node: &GraphNodeCurrent) -> bool {
    graph_node_executable_kind(node).as_deref() == Some(INLET_KIND)
}

fn is_outlet_node(node: &GraphNodeCurrent) -> bool {
    graph_node_executable_kind(node).as_deref() == Some(OUTLET_KIND)
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
            "implementation": node.implementation,
            "kind": graph_node_executable_kind(node),
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
        diagnostics.extend(object_spec_diagnostics_current(graph, node));
        if graph_node_object_id(node).is_some_and(is_payload_identity_node_kind_current) {
            diagnostics.push(payload_identity_node_kind_diagnostic_current(
                None, graph, node,
            ));
        }
        if node_has_non_resolved_object_resolution(node) {
            continue;
        }
        let kind = graph_node_executable_kind(node);
        let kind_version = graph_node_executable_kind_version(node);
        match kind
            .as_deref()
            .zip(kind_version.as_deref())
            .and_then(|key| registry.get(&key))
        {
            Some(definition) => validate_node_snapshot_current(node, definition, &mut diagnostics),
            None => diagnostics.push(RuntimeDiagnostic::structured_error(
                "node-definition.missing",
                format!(
                    "missing node definition: {}@{}",
                    kind.as_deref().unwrap_or("<missing-implementation>"),
                    kind_version.as_deref().unwrap_or(CURRENT_OBJECT_VERSION)
                ),
                json!({
                    "surface": "node-definition",
                    "nodeId": node.id,
                    "implementation": node.implementation,
                    "kind": kind,
                    "kindVersion": kind_version,
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

fn node_has_non_resolved_object_resolution(node: &GraphNodeCurrent) -> bool {
    node.object_resolution.as_ref().is_some_and(|resolution| {
        resolution.status != crate::ObjectResolutionStatusCurrent::Resolved
    })
}

pub(crate) fn is_payload_identity_node_kind_current(kind: &str) -> bool {
    is_payload_identity_kind(kind)
}

fn object_spec_diagnostics_current(
    graph: &GraphDocumentCurrent,
    node: &GraphNodeCurrent,
) -> Vec<RuntimeDiagnostic> {
    let Some(object_spec) = node_object_spec(node) else {
        return Vec::new();
    };
    let resolution = resolve_object_spec_v01(&object_spec);
    let mut diagnostics = resolution
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code != "object-spec.unresolved")
        .map(|diagnostic| {
            RuntimeDiagnostic::structured_error(
                diagnostic.code.clone(),
                diagnostic.message.clone(),
                json!({
                    "surface": "object-spec",
                    "graphId": graph.id,
                    "nodeId": node.id,
                    "implementation": node.implementation,
                    "objectSpec": object_spec,
                    "classSymbol": resolution.class_symbol,
                }),
            )
        })
        .collect::<Vec<_>>();

    if diagnostics.is_empty()
        && let Some(resolved_implementation) = resolution.implementation.as_ref()
        && let Some(node_implementation) = node.implementation.as_ref()
        && resolved_implementation != node_implementation
        && !is_subpatch_node(node)
    {
        diagnostics.push(RuntimeDiagnostic::structured_error(
            "object-spec.implementation-mismatch",
            format!(
                "object spec {} resolves to implementation {}, but node {} uses implementation {}",
                object_spec,
                resolved_implementation.object_id,
                node.id,
                node_implementation.object_id
            ),
            json!({
                "surface": "object-spec",
                "graphId": graph.id,
                "nodeId": node.id,
                "objectSpec": object_spec,
                "classSymbol": resolution.class_symbol,
                "resolvedImplementation": resolved_implementation,
                "nodeImplementation": node_implementation,
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
        "implementation": node.implementation,
        "objectId": graph_node_object_id(node),
    });
    if let Some(patch_id) = patch_id {
        details["patchId"] = json!(patch_id);
    }

    RuntimeDiagnostic::structured_error(
        "graph.payload-node-kind",
        format!(
            "node {} uses payload identity {} as an executable implementation",
            node.id,
            graph_node_object_id(node).unwrap_or("<missing>")
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
        let kind =
            graph_node_executable_kind(node).unwrap_or_else(|| "object.core.unresolved".to_owned());
        let kind_version = graph_node_executable_kind_version(node)
            .unwrap_or_else(|| CURRENT_OBJECT_VERSION.to_owned());
        let definition = registry
            .get(&(kind.as_str(), kind_version.as_str()))
            .expect("current 0.1 validation should resolve definitions");
        let execution_model = map_execution_model_current(&definition.execution.model);
        plan_nodes.push(PlanNode {
            node_id: node.id.clone(),
            kind,
            kind_version,
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

        let connection_policy = port_connection_policy(source, target);
        if !connection_policy.accepted && connection_policy.reason != "direction-mismatch" {
            diagnostics.push(edge_diagnostic(
                "graph.edge-incompatible-type",
                format!(
                    "incompatible edge {}:{} {} -> {}:{} {} ({})",
                    edge.source.node_id,
                    edge.source.port_id,
                    source.port_type,
                    edge.target.node_id,
                    edge.target.port_id,
                    target.port_type,
                    connection_policy.reason
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
            "implementation": node.implementation,
            "kind": graph_node_executable_kind(node),
            "kindVersion": graph_node_executable_kind_version(node),
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
mod tests;
