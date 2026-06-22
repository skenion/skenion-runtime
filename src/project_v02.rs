use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use skenion_contracts::EdgeEndpointV02;

use crate::{
    CycleValidationV02, EdgeSpecV02, ExecutionGroup, ExecutionModel, ExecutionModelV02,
    FanOutPolicyV02, GraphDocumentV02, GraphNodeV02, GraphValidationResultV02, MergePolicyV02,
    NodeDefinitionV02, PatchDefinitionV02, PlanEdge, PlanEdgeMetadata, PlanNode,
    ProjectDocumentV02, RuntimeDiagnostic, ViewState,
};

const SUBPATCH_KIND: &str = "core.subpatch";
const SUBPATCH_SHORTHAND_KIND: &str = "p";
const INLET_KIND: &str = "core.inlet";
const OUTLET_KIND: &str = "core.outlet";
const MAX_SUBPATCH_DEPTH: usize = 16;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRequestV02 {
    #[serde(skip)]
    pub document: Option<ProjectDocumentV02>,
    pub graph: GraphDocumentV02,
    #[serde(default)]
    pub nodes: Vec<NodeDefinitionV02>,
    #[serde(default)]
    pub patch_library: Vec<PatchDefinitionV02>,
    #[serde(default)]
    pub view_state: Option<ViewState>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunProjectRequestV02 {
    #[serde(skip)]
    pub document: Option<ProjectDocumentV02>,
    pub graph: GraphDocumentV02,
    #[serde(default)]
    pub nodes: Vec<NodeDefinitionV02>,
    #[serde(default)]
    pub patch_library: Vec<PatchDefinitionV02>,
    #[serde(default)]
    pub view_state: Option<ViewState>,
    pub frames: Option<usize>,
}

impl From<ProjectDocumentV02> for ProjectRequestV02 {
    fn from(document: ProjectDocumentV02) -> Self {
        Self {
            graph: document.graph.clone(),
            nodes: Vec::new(),
            patch_library: document.patch_library.clone(),
            view_state: Some(document.view_state.clone()),
            document: Some(document),
        }
    }
}

impl From<ProjectDocumentV02> for RunProjectRequestV02 {
    fn from(document: ProjectDocumentV02) -> Self {
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

impl ProjectRequestV02 {
    pub fn from_project_document(
        document: ProjectDocumentV02,
        nodes: Vec<NodeDefinitionV02>,
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

impl RunProjectRequestV02 {
    pub fn from_project_document(
        document: ProjectDocumentV02,
        nodes: Vec<NodeDefinitionV02>,
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

type V02Validation =
    Result<(Vec<RuntimeDiagnostic>, GraphValidationResultV02), Vec<RuntimeDiagnostic>>;

#[derive(Debug, Clone)]
struct ExpandedGraphV02 {
    nodes: Vec<GraphNodeV02>,
    edges: Vec<ExpansionEdge>,
    boundary_pins: HashSet<String>,
    inlets: HashMap<String, String>,
    outlets: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct ExpansionEdge {
    edge: EdgeSpecV02,
    source: ExpansionEndpoint,
    target: ExpansionEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExpansionEndpoint {
    Node(EdgeEndpointV02),
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
    patches: HashMap<&'a str, &'a PatchDefinitionV02>,
    diagnostics: Vec<RuntimeDiagnostic>,
}

pub fn expand_project_graph_v02(
    graph: &GraphDocumentV02,
    patch_library: &[PatchDefinitionV02],
) -> Result<GraphDocumentV02, Vec<RuntimeDiagnostic>> {
    let mut context = ExpansionContext {
        patches: patch_library
            .iter()
            .map(|definition| (definition.id.as_str(), definition))
            .collect(),
        diagnostics: Vec::new(),
    };
    let expanded = expand_graph_v02(graph, "", 0, &[], &mut context);

    if !context.diagnostics.is_empty() {
        return Err(context.diagnostics);
    }

    Ok(GraphDocumentV02 {
        schema: graph.schema.clone(),
        schema_version: graph.schema_version.clone(),
        id: graph.id.clone(),
        revision: graph.revision.clone(),
        nodes: expanded.nodes,
        edges: contract_boundary_edges(expanded.edges, expanded.boundary_pins),
        cable_styles: graph.cable_styles.clone(),
    })
}

pub fn validate_project_request_v02(request: &ProjectRequestV02) -> V02Validation {
    validate_patch_library_v02(&request.patch_library)?;
    let graph = expand_project_graph_v02(&request.graph, &request.patch_library)?;
    validate_project_v02(&graph, &request.nodes)
}

pub fn build_execution_plan_request_v02(
    request: &ProjectRequestV02,
) -> Result<(crate::ExecutionPlan, Vec<RuntimeDiagnostic>), Vec<RuntimeDiagnostic>> {
    validate_patch_library_v02(&request.patch_library)?;
    let graph = expand_project_graph_v02(&request.graph, &request.patch_library)?;
    build_execution_plan_v02(&graph, &request.nodes)
}

pub fn build_execution_plan_run_request_v02(
    request: &RunProjectRequestV02,
) -> Result<(crate::ExecutionPlan, Vec<RuntimeDiagnostic>), Vec<RuntimeDiagnostic>> {
    validate_patch_library_v02(&request.patch_library)?;
    let graph = expand_project_graph_v02(&request.graph, &request.patch_library)?;
    build_execution_plan_v02(&graph, &request.nodes)
}

fn validate_patch_library_v02(
    patch_library: &[PatchDefinitionV02],
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

        if let Err(report) = skenion_contracts::validate_patch_definition_v02(patch) {
            diagnostics.extend(report.errors().iter().map(|error| {
                RuntimeDiagnostic::structured_error(
                    "subpatch.invalid-patch-definition",
                    error.message.clone(),
                    json!({ "patchId": patch.id }),
                )
            }));
        }
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn expand_graph_v02(
    graph: &GraphDocumentV02,
    namespace: &str,
    depth: usize,
    stack: &[String],
    context: &mut ExpansionContext<'_>,
) -> ExpandedGraphV02 {
    let mut expanded = ExpandedGraphV02 {
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
            let child = expand_graph_v02(
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
) -> Vec<EdgeSpecV02> {
    let mut counter = 0usize;

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
                counter += 1;
                retained.push(merge_boundary_edges(
                    source_edge,
                    target_edge,
                    &pin,
                    counter,
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
    counter: usize,
) -> ExpansionEdge {
    let mut edge = target_edge.edge.clone();
    edge.id = format!(
        "{}__{}__{}",
        source_edge.edge.id,
        boundary_id_fragment(pin),
        counter
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

fn expansion_edge_to_real_edge(expansion: ExpansionEdge) -> Option<EdgeSpecV02> {
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
    edge: &EdgeSpecV02,
    namespace: &str,
    nodes: &HashMap<String, NodeExpansion>,
    context: &mut ExpansionContext<'_>,
) -> ExpansionEndpoint {
    match nodes.get(&edge.source.node_id) {
        Some(NodeExpansion::Node(node_id)) => ExpansionEndpoint::Node(EdgeEndpointV02 {
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
                ExpansionEndpoint::Node(EdgeEndpointV02 {
                    node_id: namespaced_id(namespace, &edge.source.node_id),
                    port_id: edge.source.port_id.clone(),
                })
            }),
        None => ExpansionEndpoint::Node(EdgeEndpointV02 {
            node_id: namespaced_id(namespace, &edge.source.node_id),
            port_id: edge.source.port_id.clone(),
        }),
    }
}

fn resolve_target_endpoint(
    edge: &EdgeSpecV02,
    namespace: &str,
    nodes: &HashMap<String, NodeExpansion>,
    context: &mut ExpansionContext<'_>,
) -> ExpansionEndpoint {
    match nodes.get(&edge.target.node_id) {
        Some(NodeExpansion::Node(node_id)) => ExpansionEndpoint::Node(EdgeEndpointV02 {
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
                ExpansionEndpoint::Node(EdgeEndpointV02 {
                    node_id: namespaced_id(namespace, &edge.target.node_id),
                    port_id: edge.target.port_id.clone(),
                })
            }),
        None => ExpansionEndpoint::Node(EdgeEndpointV02 {
            node_id: namespaced_id(namespace, &edge.target.node_id),
            port_id: edge.target.port_id.clone(),
        }),
    }
}

fn register_boundary_node(
    node: &GraphNodeV02,
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

fn boundary_aliases(node: &GraphNodeV02, key: &str) -> Vec<String> {
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

fn boundary_key(node: &GraphNodeV02) -> String {
    ["portId", "port", "name", "id", "label"]
        .into_iter()
        .find_map(|key| string_param(&node.params, key))
        .unwrap_or_else(|| node.id.clone())
}

fn subpatch_ref(node: &GraphNodeV02) -> Option<String> {
    ["patchRef", "patchId", "patch", "ref", "name", "id"]
        .into_iter()
        .find_map(|key| string_param(&node.params, key))
        .or_else(|| {
            ["objectText", "sourceText", "text"]
                .into_iter()
                .find_map(|key| string_param(&node.params, key))
                .and_then(|text| parse_subpatch_object_text(&text))
        })
}

fn parse_subpatch_object_text(text: &str) -> Option<String> {
    let mut parts = text.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("p" | "core.subpatch"), Some(patch_ref)) => Some(patch_ref.to_owned()),
        _ => None,
    }
}

fn string_param(params: &Map<String, Value>, key: &str) -> Option<String> {
    match params.get(key)? {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn is_subpatch_node(node: &GraphNodeV02) -> bool {
    matches!(node.kind.as_str(), SUBPATCH_KIND | SUBPATCH_SHORTHAND_KIND)
}

fn is_inlet_node(node: &GraphNodeV02) -> bool {
    node.kind == INLET_KIND
}

fn is_outlet_node(node: &GraphNodeV02) -> bool {
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
    node: &GraphNodeV02,
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
    node: &GraphNodeV02,
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

pub fn validate_project_v02(
    graph: &GraphDocumentV02,
    nodes: &[NodeDefinitionV02],
) -> V02Validation {
    let mut diagnostics = Vec::new();
    let mut registry: HashMap<(&str, &str), &NodeDefinitionV02> = HashMap::new();

    for definition in nodes {
        if let Err(report) = skenion_contracts::validate_node_definition_v02(definition) {
            diagnostics.extend(
                report
                    .errors()
                    .iter()
                    .map(|error| RuntimeDiagnostic::error(error.message.clone())),
            );
        }
        registry.insert(
            (definition.id.as_str(), definition.version.as_str()),
            definition,
        );
    }

    let graph_analysis = skenion_contracts::analyze_graph_document_v02(graph);
    diagnostics.extend(graph_analysis.diagnostics.iter().map(|diagnostic| {
        let message = format!("{}: {}", diagnostic.code, diagnostic.message);
        if diagnostic.severity == "warning" {
            RuntimeDiagnostic::warning(message)
        } else {
            RuntimeDiagnostic::error(message)
        }
    }));

    for node in &graph.nodes {
        match registry.get(&(node.kind.as_str(), node.kind_version.as_str())) {
            Some(definition) => validate_node_snapshot_v02(node, definition, &mut diagnostics),
            None => diagnostics.push(RuntimeDiagnostic::error(format!(
                "missing node definition: {}@{}",
                node.kind, node.kind_version
            ))),
        }
    }

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == crate::DiagnosticSeverity::Error)
    {
        Err(diagnostics)
    } else {
        Ok((diagnostics, graph_analysis))
    }
}

pub fn build_execution_plan_v02(
    graph: &GraphDocumentV02,
    nodes: &[NodeDefinitionV02],
) -> Result<(crate::ExecutionPlan, Vec<RuntimeDiagnostic>), Vec<RuntimeDiagnostic>> {
    let (diagnostics, analysis) = validate_project_v02(graph, nodes)?;
    let registry = nodes
        .iter()
        .map(|definition| {
            (
                (definition.id.as_str(), definition.version.as_str()),
                definition,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut groups_by_model: BTreeMap<String, ExecutionGroup> = BTreeMap::new();
    let mut plan_nodes = Vec::new();

    for (order, node) in graph.nodes.iter().enumerate() {
        let definition = registry
            .get(&(node.kind.as_str(), node.kind_version.as_str()))
            .expect("v0.2 validation should resolve definitions");
        let execution_model = map_execution_model_v02(&definition.execution.model);
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
                .map(|edge| plan_edge_v02(graph, edge, &analysis))
                .collect(),
            groups: groups_by_model.into_values().collect(),
        },
        diagnostics,
    ))
}

fn validate_node_snapshot_v02(
    node: &crate::GraphNodeV02,
    definition: &NodeDefinitionV02,
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
            diagnostics.push(RuntimeDiagnostic::error(format!(
                "port snapshot missing manifest port: {}.{}",
                node.id, definition_port.id
            )));
        }
    }

    for snapshot_port in &node.ports {
        let Some(definition_port) = definition_ports.get(snapshot_port.id.as_str()) else {
            diagnostics.push(RuntimeDiagnostic::error(format!(
                "port snapshot references missing manifest port: {}.{}",
                node.id, snapshot_port.id
            )));
            continue;
        };

        if snapshot_port.direction != definition_port.direction {
            diagnostics.push(RuntimeDiagnostic::error(format!(
                "port snapshot mismatch: {}.{} direction differs from definition",
                node.id, snapshot_port.id
            )));
        }
        if snapshot_port.port_type != definition_port.port_type {
            diagnostics.push(RuntimeDiagnostic::error(format!(
                "port snapshot mismatch: {}.{} type {} != definition type {}",
                node.id, snapshot_port.id, snapshot_port.port_type, definition_port.port_type
            )));
        }
    }
}

fn plan_edge_v02(
    graph: &GraphDocumentV02,
    edge: &EdgeSpecV02,
    analysis: &GraphValidationResultV02,
) -> PlanEdge {
    let source = find_port(graph, &edge.source.node_id, &edge.source.port_id)
        .expect("v0.2 validation should resolve source port");
    let target = find_port(graph, &edge.target.node_id, &edge.target.port_id)
        .expect("v0.2 validation should resolve target port");

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
    graph: &'a GraphDocumentV02,
    node_id: &str,
    port_id: &str,
) -> Option<&'a crate::PortSpecV02> {
    graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)?
        .ports
        .iter()
        .find(|port| port.id == port_id)
}

fn cycle_classification_for_edge(
    edge: &EdgeSpecV02,
    analysis: &GraphValidationResultV02,
) -> Option<String> {
    analysis
        .cycles
        .iter()
        .find(|cycle| cycle.edges.iter().any(|edge_id| edge_id == &edge.id))
        .map(|cycle| cycle_validation_label(&cycle.classification).to_owned())
}

fn cycle_validation_label(classification: &CycleValidationV02) -> &'static str {
    match classification {
        CycleValidationV02::NoCycle => "no-cycle",
        CycleValidationV02::ValidFeedback => "valid-feedback",
        CycleValidationV02::RiskyFeedback => "risky-feedback",
        CycleValidationV02::AmbiguousAlgebraicLoop => "ambiguous-algebraic-loop",
        CycleValidationV02::InvalidCycle => "invalid-cycle",
    }
}

fn merge_policy_label(policy: Option<&MergePolicyV02>) -> String {
    match policy {
        Some(MergePolicyV02::OrderedEvents) => "ordered-events",
        Some(MergePolicyV02::Mix) => "mix",
        Some(MergePolicyV02::Array) => "array",
        Some(MergePolicyV02::Latest) => "latest",
        Some(MergePolicyV02::First) => "first",
        Some(MergePolicyV02::Custom) => "custom",
        Some(MergePolicyV02::Forbid) | None => "forbid",
    }
    .to_owned()
}

fn fan_out_policy_label(policy: Option<&FanOutPolicyV02>) -> String {
    match policy {
        Some(FanOutPolicyV02::Forbid) => "forbid",
        Some(FanOutPolicyV02::Copy) => "copy",
        Some(FanOutPolicyV02::Share) => "share",
        Some(FanOutPolicyV02::Allow) | None => "allow",
    }
    .to_owned()
}

fn map_execution_model_v02(model: &ExecutionModelV02) -> ExecutionModel {
    match model {
        ExecutionModelV02::Event => ExecutionModel::Event,
        ExecutionModelV02::Value => ExecutionModel::Value,
        ExecutionModelV02::Frame => ExecutionModel::Frame,
        ExecutionModelV02::AudioBlock => ExecutionModel::AudioBlock,
        ExecutionModelV02::VideoFrame => ExecutionModel::VideoFrame,
        ExecutionModelV02::GpuPass => ExecutionModel::GpuPass,
        ExecutionModelV02::AsyncResource => ExecutionModel::AsyncResource,
        ExecutionModelV02::ScriptControl => ExecutionModel::ScriptControl,
        ExecutionModelV02::NativePlugin => ExecutionModel::NativePlugin,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{DiagnosticSeverity, FeedbackBoundaryV02, PortDirectionV02, PortSpecV02};

    fn graph(value: Value) -> GraphDocumentV02 {
        serde_json::from_value(value).expect("graph should parse")
    }

    fn definition(value: Value) -> NodeDefinitionV02 {
        serde_json::from_value(value).expect("definition should parse")
    }

    fn clear_definition() -> NodeDefinitionV02 {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.2.0",
          "id": "render.clear-color",
          "version": "0.2.0",
          "displayName": "Clear Color",
          "category": "Render",
          "ports": [
            { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn output_definition() -> NodeDefinitionV02 {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.2.0",
          "id": "render.output",
          "version": "0.2.0",
          "displayName": "Render Output",
          "category": "Render",
          "ports": [
            { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn pass_definition() -> NodeDefinitionV02 {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.2.0",
          "id": "test.pass",
          "version": "0.2.0",
          "displayName": "Pass",
          "category": "Test",
          "ports": [
            { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true },
            { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn render_graph() -> GraphDocumentV02 {
        graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.2.0",
          "id": "render",
          "revision": "1",
          "nodes": [
            {
              "id": "clear",
              "kind": "render.clear-color",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
              ]
            },
            {
              "id": "output",
              "kind": "render.output",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true }
              ]
            }
          ],
          "edges": [
            {
              "id": "edge_clear_output",
              "source": { "nodeId": "clear", "portId": "out" },
              "target": { "nodeId": "output", "portId": "in" },
              "resolvedType": "render.frame"
            }
          ]
        }))
    }

    fn identity_patch() -> PatchDefinitionV02 {
        serde_json::from_value(json!({
          "id": "identity",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.2.0",
            "id": "identity-graph",
            "revision": "1",
            "nodes": [
              {
                "id": "patch_in",
                "kind": "core.inlet",
                "kindVersion": "0.2.0",
                "params": { "portId": "in", "label": "Input" },
                "ports": [
                  { "id": "out", "direction": "output", "type": "render.frame", "rate": "render", "description": "Frame entering the patch" }
                ]
              },
              {
                "id": "pass",
                "kind": "test.pass",
                "kindVersion": "0.2.0",
                "params": {},
                "ports": [
                  { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true },
                  { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
                ]
              },
              {
                "id": "patch_out",
                "kind": "core.outlet",
                "kindVersion": "0.2.0",
                "params": { "portId": "out", "label": "Output" },
                "ports": [
                  { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true, "description": "Frame leaving the patch" }
                ]
              }
            ],
            "edges": [
              {
                "id": "edge_in_pass",
                "source": { "nodeId": "patch_in", "portId": "out" },
                "target": { "nodeId": "pass", "portId": "in" },
                "resolvedType": "render.frame"
              },
              {
                "id": "edge_pass_out",
                "source": { "nodeId": "pass", "portId": "out" },
                "target": { "nodeId": "patch_out", "portId": "in" },
                "resolvedType": "render.frame"
              }
            ]
          }
        }))
        .expect("patch definition should parse")
    }

    fn subpatch_graph() -> GraphDocumentV02 {
        graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.2.0",
          "id": "render-subpatch",
          "revision": "1",
          "nodes": [
            {
              "id": "clear",
              "kind": "render.clear-color",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
              ]
            },
            {
              "id": "fx",
              "kind": "core.subpatch",
              "kindVersion": "0.2.0",
              "params": { "patchRef": "identity" },
              "ports": [
                { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true },
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
              ]
            },
            {
              "id": "output",
              "kind": "render.output",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true }
              ]
            }
          ],
          "edges": [
            {
              "id": "edge_clear_fx",
              "source": { "nodeId": "clear", "portId": "out" },
              "target": { "nodeId": "fx", "portId": "in" },
              "resolvedType": "render.frame"
            },
            {
              "id": "edge_fx_output",
              "source": { "nodeId": "fx", "portId": "out" },
              "target": { "nodeId": "output", "portId": "in" },
              "resolvedType": "render.frame"
            }
          ]
        }))
    }

    fn project_document() -> ProjectDocumentV02 {
        serde_json::from_value(json!({
          "schema": "skenion.project",
          "schemaVersion": "0.2.0",
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
    fn validates_and_builds_v02_plan_metadata() {
        let graph = render_graph();
        let nodes = vec![clear_definition(), output_definition()];
        let (warnings, analysis) =
            validate_project_v02(&graph, &nodes).expect("project should validate");
        assert!(warnings.is_empty());
        assert!(analysis.cycles.is_empty());

        let (plan, diagnostics) =
            build_execution_plan_v02(&graph, &nodes).expect("plan should build");
        assert!(diagnostics.is_empty());
        assert_eq!(plan.graph_id, "render");
        assert_eq!(plan.nodes.len(), 2);
        assert_eq!(plan.groups.len(), 1);
        let metadata = plan.edges[0]
            .metadata
            .as_ref()
            .expect("metadata should exist");
        assert_eq!(metadata.resolved_type.as_deref(), Some("render.frame"));
        assert_eq!(metadata.merge_policy.as_deref(), Some("forbid"));
        assert_eq!(metadata.fan_out_policy.as_deref(), Some("allow"));
        assert_eq!(metadata.cycle_classification, None);
    }

    #[test]
    fn records_merge_order_feedback_and_risky_warnings() {
        let mut graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.2.0",
          "id": "feedback",
          "revision": "1",
          "nodes": [
            {
              "id": "node",
              "kind": "render.feedback-composite",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": [
                { "id": "previous", "direction": "input", "type": "render.frame", "rate": "render" },
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render", "fanOutPolicy": "copy" }
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
          "schemaVersion": "0.2.0",
          "id": "render.feedback-composite",
          "version": "0.2.0",
          "displayName": "Feedback",
          "category": "Render",
          "ports": [
            { "id": "previous", "direction": "input", "type": "render.frame", "rate": "render" },
            { "id": "out", "direction": "output", "type": "render.frame", "rate": "render", "fanOutPolicy": "copy" }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }));

        let (plan, diagnostics) =
            build_execution_plan_v02(&graph, std::slice::from_ref(&definition))
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
            FeedbackBoundaryV02::RenderFrame
        );

        graph.edges[0].feedback.as_mut().unwrap().boundary = FeedbackBoundaryV02::SameTurn;
        let (_plan, diagnostics) =
            build_execution_plan_v02(&graph, &[definition]).expect("risky feedback should plan");
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Warning);
        assert!(diagnostics[0].message.contains("risky-feedback"));
    }

    #[test]
    fn project_document_conversions_default_runtime_fields() {
        let document = project_document();

        let request: ProjectRequestV02 = document.clone().into();
        assert_eq!(request.graph.id, "render-subpatch");
        assert!(request.nodes.is_empty());
        assert_eq!(request.patch_library[0].id, "identity");

        let run_request: RunProjectRequestV02 = document.into();
        assert_eq!(run_request.graph.id, "render-subpatch");
        assert!(run_request.nodes.is_empty());
        assert_eq!(run_request.patch_library[0].id, "identity");
        assert_eq!(run_request.frames, None);
    }

    #[test]
    fn expands_subpatches_before_v02_validation_and_planning() {
        let request = ProjectRequestV02 {
            document: None,
            graph: subpatch_graph(),
            nodes: vec![clear_definition(), output_definition(), pass_definition()],
            patch_library: vec![identity_patch()],
            view_state: None,
        };

        let expanded = expand_project_graph_v02(&request.graph, &request.patch_library)
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
            validate_project_request_v02(&request).expect("expanded project should validate");
        assert!(diagnostics.is_empty());
        let (plan, diagnostics) =
            build_execution_plan_request_v02(&request).expect("expanded project should plan");
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
    fn contracts_boundary_edges_and_filters_boundary_only_edges() {
        let base_edge = render_graph().edges[0].clone();
        let endpoint = EdgeEndpointV02 {
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
        source_edge.resolved_type = Some("render.frame".to_owned());
        let mut target_edge = base_edge.clone();
        target_edge.id = "target_edge".to_owned();
        target_edge.resolved_type = None;
        let merged = contract_boundary_edges(
            vec![
                ExpansionEdge {
                    edge: source_edge,
                    source: ExpansionEndpoint::Node(EdgeEndpointV02 {
                        node_id: "source".to_owned(),
                        port_id: "out".to_owned(),
                    }),
                    target: ExpansionEndpoint::Boundary("fx::@inlet::in".to_owned()),
                },
                ExpansionEdge {
                    edge: target_edge,
                    source: ExpansionEndpoint::Boundary("fx::@inlet::in".to_owned()),
                    target: ExpansionEndpoint::Node(EdgeEndpointV02 {
                        node_id: "target".to_owned(),
                        port_id: "in".to_owned(),
                    }),
                },
            ],
            std::collections::HashSet::from(["fx::@inlet::in".to_owned()]),
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].resolved_type.as_deref(), Some("render.frame"));
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
        let duplicate = ProjectRequestV02 {
            document: None,
            graph: render_graph(),
            nodes: vec![clear_definition(), output_definition()],
            patch_library: vec![identity_patch(), identity_patch()],
            view_state: None,
        };
        let duplicate_diagnostics =
            validate_project_request_v02(&duplicate).expect_err("duplicate patch ids should fail");
        assert_eq!(
            duplicate_diagnostics[0].code.as_deref(),
            Some("subpatch.duplicate-patch-id")
        );

        let missing_ref = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.2.0",
          "id": "missing-ref",
          "revision": "1",
          "nodes": [
            {
              "id": "fx",
              "kind": "core.subpatch",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": []
            }
          ],
          "edges": []
        }));
        let missing_ref_diagnostics =
            expand_project_graph_v02(&missing_ref, &[]).expect_err("missing ref should fail");
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
                    "schemaVersion": "0.2.0",
                    "id": format!("p{index}-graph"),
                    "revision": "1",
                    "nodes": [
                      {
                        "id": "next",
                        "kind": "core.subpatch",
                        "kindVersion": "0.2.0",
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
          "schemaVersion": "0.2.0",
          "id": "depth-root",
          "revision": "1",
          "nodes": [
            {
              "id": "root",
              "kind": "core.subpatch",
              "kindVersion": "0.2.0",
              "params": { "patchRef": "p0" },
              "ports": []
            }
          ],
          "edges": []
        }));
        let depth_diagnostics =
            expand_project_graph_v02(&depth_root, &patch_library).expect_err("depth should fail");
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
            parse_subpatch_object_text("core.subpatch identity").as_deref(),
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
          "schemaVersion": "0.2.0",
          "id": "fallback-boundary",
          "revision": "1",
          "nodes": [
            {
              "id": "plain_inlet",
              "kind": "core.inlet",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": []
            }
          ],
          "edges": []
        }));
        assert_eq!(boundary_key(&fallback_boundary.nodes[0]), "plain_inlet");

        let duplicate_inlet_patch: PatchDefinitionV02 = serde_json::from_value(json!({
          "id": "alias-patch",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.2.0",
            "id": "alias-patch-graph",
            "revision": "1",
            "nodes": [
              {
                "id": "in_a",
                "kind": "core.inlet",
                "kindVersion": "0.2.0",
                "params": { "portId": "in_a", "label": "shared" },
                "ports": [
                  { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
                ]
              },
              {
                "id": "in_b",
                "kind": "core.inlet",
                "kindVersion": "0.2.0",
                "params": { "portId": "in_b", "label": "shared" },
                "ports": [
                  { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
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
          "schemaVersion": "0.2.0",
          "id": "alias-root",
          "revision": "1",
          "nodes": [
            {
              "id": "clear",
              "kind": "render.clear-color",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
              ]
            },
            {
              "id": "fx",
              "kind": "p",
              "kindVersion": "0.2.0",
              "params": { "objectText": "p alias-patch" },
              "ports": [
                { "id": "in", "direction": "input", "type": "render.frame", "rate": "render" },
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
              ]
            },
            {
              "id": "output",
              "kind": "render.output",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": "render.frame", "rate": "render" }
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
        let diagnostics =
            expand_project_graph_v02(&root, &[duplicate_inlet_patch]).expect_err("boundaries fail");
        let codes = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_deref())
            .collect::<Vec<_>>();
        assert!(codes.contains(&Some("subpatch.missing-inlet")));
        assert!(codes.contains(&Some("subpatch.missing-outlet")));
    }

    #[test]
    fn reports_missing_recursive_and_invalid_patch_library_diagnostics() {
        let missing = ProjectRequestV02 {
            document: None,
            graph: subpatch_graph(),
            nodes: vec![clear_definition(), output_definition(), pass_definition()],
            patch_library: Vec::new(),
            view_state: None,
        };
        let missing_diagnostics =
            validate_project_request_v02(&missing).expect_err("missing patch should fail");
        assert_eq!(
            missing_diagnostics[0].code.as_deref(),
            Some("subpatch.missing-patch")
        );

        let recursive_patch: PatchDefinitionV02 = serde_json::from_value(json!({
          "id": "recursive",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.2.0",
            "id": "recursive-graph",
            "revision": "1",
            "nodes": [
              {
                "id": "self",
                "kind": "core.subpatch",
                "kindVersion": "0.2.0",
                "params": { "patchRef": "recursive" },
                "ports": []
              }
            ],
            "edges": []
          }
        }))
        .expect("recursive patch should parse");
        let recursive = ProjectRequestV02 {
            document: None,
            graph: graph(json!({
              "schema": "skenion.graph",
              "schemaVersion": "0.2.0",
              "id": "recursive-root",
              "revision": "1",
              "nodes": [
                {
                  "id": "root",
                  "kind": "core.subpatch",
                  "kindVersion": "0.2.0",
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
            validate_project_request_v02(&recursive).expect_err("recursive patch should fail");
        assert_eq!(
            recursive_diagnostics[0].code.as_deref(),
            Some("subpatch.recursion")
        );

        let mut duplicate_boundary = identity_patch();
        duplicate_boundary.graph.nodes[2].params["portId"] = json!("in");
        let invalid = ProjectRequestV02 {
            document: None,
            graph: render_graph(),
            nodes: vec![clear_definition(), output_definition()],
            patch_library: vec![duplicate_boundary],
            view_state: None,
        };
        let invalid_diagnostics =
            validate_project_request_v02(&invalid).expect_err("invalid patch should fail");
        assert_eq!(
            invalid_diagnostics[0].code.as_deref(),
            Some("subpatch.invalid-patch-definition")
        );
    }

    #[test]
    fn rejects_invalid_graph_definitions_and_snapshots() {
        let graph = render_graph();
        let missing = validate_project_v02(&graph, &[]).expect_err("missing definitions fail");
        assert!(missing[0].message.contains("missing node definition"));

        let mut invalid_definition = clear_definition();
        invalid_definition.permissions.push("network".to_owned());
        let invalid_definition_result =
            validate_project_v02(&graph, &[invalid_definition, output_definition()])
                .expect_err("invalid definition should fail");
        assert!(
            invalid_definition_result
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unsupported permission"))
        );

        let mut mismatch = render_graph();
        mismatch.nodes[0].ports.clear();
        mismatch.nodes[1].ports[0].direction = PortDirectionV02::Output;
        mismatch.nodes[1].ports[0].port_type = "value.number".to_owned();
        mismatch.nodes[1].ports.push(PortSpecV02 {
            id: "extra".to_owned(),
            direction: PortDirectionV02::Input,
            port_type: "render.frame".to_owned(),
            label: None,
            rate: None,
            accepts: None,
            min_connections: None,
            max_connections: None,
            merge_policy: None,
            fan_out_policy: None,
            trigger_mode: None,
            default_value: None,
            latch: None,
            required: None,
            style_key: None,
            group: None,
            description: None,
        });
        let mismatch_result =
            validate_project_v02(&mismatch, &[clear_definition(), output_definition()])
                .expect_err("snapshot mismatch should fail");
        let messages = mismatch_result
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(messages.contains("missing manifest port"));
        assert!(messages.contains("direction differs from definition"));
        assert!(messages.contains("type value.number"));
        assert!(messages.contains("missing source port"));
        assert!(messages.contains("missing manifest port: output.extra"));
    }

    #[test]
    fn labels_all_v02_policy_and_execution_variants() {
        for (policy, expected) in [
            (Some(MergePolicyV02::Forbid), "forbid"),
            (Some(MergePolicyV02::OrderedEvents), "ordered-events"),
            (Some(MergePolicyV02::Mix), "mix"),
            (Some(MergePolicyV02::Array), "array"),
            (Some(MergePolicyV02::Latest), "latest"),
            (Some(MergePolicyV02::First), "first"),
            (Some(MergePolicyV02::Custom), "custom"),
            (None, "forbid"),
        ] {
            assert_eq!(merge_policy_label(policy.as_ref()), expected);
        }

        for (policy, expected) in [
            (Some(FanOutPolicyV02::Allow), "allow"),
            (Some(FanOutPolicyV02::Forbid), "forbid"),
            (Some(FanOutPolicyV02::Copy), "copy"),
            (Some(FanOutPolicyV02::Share), "share"),
            (None, "allow"),
        ] {
            assert_eq!(fan_out_policy_label(policy.as_ref()), expected);
        }

        for (classification, expected) in [
            (CycleValidationV02::NoCycle, "no-cycle"),
            (CycleValidationV02::ValidFeedback, "valid-feedback"),
            (CycleValidationV02::RiskyFeedback, "risky-feedback"),
            (
                CycleValidationV02::AmbiguousAlgebraicLoop,
                "ambiguous-algebraic-loop",
            ),
            (CycleValidationV02::InvalidCycle, "invalid-cycle"),
        ] {
            assert_eq!(cycle_validation_label(&classification), expected);
        }

        for (model, expected) in [
            (ExecutionModelV02::Event, ExecutionModel::Event),
            (ExecutionModelV02::Value, ExecutionModel::Value),
            (ExecutionModelV02::Frame, ExecutionModel::Frame),
            (ExecutionModelV02::AudioBlock, ExecutionModel::AudioBlock),
            (ExecutionModelV02::VideoFrame, ExecutionModel::VideoFrame),
            (ExecutionModelV02::GpuPass, ExecutionModel::GpuPass),
            (
                ExecutionModelV02::AsyncResource,
                ExecutionModel::AsyncResource,
            ),
            (
                ExecutionModelV02::ScriptControl,
                ExecutionModel::ScriptControl,
            ),
            (
                ExecutionModelV02::NativePlugin,
                ExecutionModel::NativePlugin,
            ),
        ] {
            assert_eq!(map_execution_model_v02(&model), expected);
        }
    }
}
