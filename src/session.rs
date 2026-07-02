use std::{
    collections::{BTreeMap, HashSet},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value, json};
use skenion_contracts::{InterfaceIncidentEdgePolicyV01, NodeCatalogSnapshotV01};

use crate::{
    CanvasNodeView, ControlState, EdgeSpecCurrent, ExecutionPlan, GraphDocument,
    GraphDocumentCurrent, GraphFragmentOutsideEndpointPolicyCurrent, GraphTargetRef,
    IdConflictPolicy, IdRemapResult, NodeDefinitionCurrent, PackageRegistryListResponseV01,
    PasteGraphFragmentRequest, PasteGraphFragmentResponse, PastePlacement, PatchPath,
    PreviewContext, ProjectDocumentCurrent, ProjectRequestCurrent, RuntimeCollaborationChange,
    RuntimeIssue, RuntimeOperationEnvelope, RuntimeOperationIssue, ViewState,
    build_execution_plan_request_current,
    project_current::{
        is_payload_identity_node_kind_current, next_graph_revision_current,
        repair_project_load_edges_current,
    },
    project_document_validation_issues_current, validate_project_request_current,
};

mod binding_formats;
mod collaboration;
mod control;
mod history;
mod node_catalog;
mod node_mutation;
mod paste;
mod planning;
mod projection;
mod types;
mod view_state;

#[cfg(test)]
use crate::GraphPatch;
use binding_formats::derive_runtime_binding_formats;
#[cfg(test)]
use binding_formats::{
    runtime_binding_format_revision, runtime_value_format_label, value_format_for_port_type,
};
#[cfg(test)]
use collaboration::apply_collaboration_changes_to_project_current;
use history::{
    HistoryApplyOutcome, HistoryDirection, HistoryEntry, project_document_history_delta,
};
#[cfg(test)]
use history::{
    redo_graph_history_delta_current, undo_graph_history_delta_current,
    view_state_history_delta_current,
};
use node_catalog::RuntimeNodeCatalogCache;
pub(crate) use node_mutation::{
    ApplyObjectNodeCreateCurrentRequest, ApplyObjectNodeReplaceCurrentRequest,
};
#[cfg(test)]
use paste::{
    lower_fragment_view_patch, next_available_edge_id, paste_graph_fragment_into_graph_current,
    paste_graph_fragment_into_project_current, remap_edge_current,
    runtime_issue_to_operation_issue,
};
use planning::unresolved_object_issues_current;
pub(crate) use projection::{lower_edge_for_execution, lower_graph_node_for_execution};
use projection::{lower_graph_for_execution, normalized_node_definitions_current};
#[cfg(test)]
use projection::{lower_port_for_execution, remap_edge};
pub use types::{
    RuntimeHistory, RuntimeHistoryEntry, RuntimeHistoryEntryKind, RuntimeMutationRequest,
    RuntimePatchResponse, RuntimeSessionResponse, RuntimeSessionSnapshot, RuntimeViewPatch,
    RuntimeViewPatchOperation,
};
#[cfg(test)]
use view_state::apply_view_patch_to_view_state;
use view_state::{
    apply_view_patch_to_view_state_current, reconcile_view_state_with_graph_current,
    runtime_owned_view_state, target_supports_view_state, unsupported_patch_view_change_issue,
};

#[derive(Debug)]
pub struct RuntimeSession {
    project: Option<ProjectDocumentCurrent>,
    nodes_current: Vec<NodeDefinitionCurrent>,
    plan: Option<ExecutionPlan>,
    view_state: Option<ViewState>,
    control_state: ControlState,
    issues: Vec<RuntimeIssue>,
    revision: u64,
    view_revision: u64,
    control_revision: u64,
    history_entries: Vec<RuntimeHistoryEntry>,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    next_event_sequence: u64,
    package_registry_revision: Option<u64>,
    package_registry: PackageRegistryListResponseV01,
    node_catalog: RuntimeNodeCatalogCache,
}

impl Default for RuntimeSession {
    fn default() -> Self {
        Self {
            project: None,
            nodes_current: Vec::new(),
            plan: None,
            view_state: None,
            control_state: ControlState::default(),
            issues: Vec::new(),
            revision: 0,
            view_revision: 0,
            control_revision: 0,
            history_entries: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            next_event_sequence: 1,
            package_registry_revision: None,
            package_registry: PackageRegistryListResponseV01 {
                ok: true,
                packages: Vec::new(),
                issues: Vec::new(),
            },
            node_catalog: RuntimeNodeCatalogCache::default(),
        }
    }
}

impl RuntimeSession {
    pub fn snapshot(&self) -> RuntimeSessionSnapshot {
        RuntimeSessionSnapshot {
            session_revision: self.revision,
            view_revision: self.view_revision,
            control_revision: self.control_revision,
            package_registry_revision: self.package_registry_revision,
            project: self.project.clone(),
            binding_formats: derive_runtime_binding_formats(self.project.as_ref()),
            issues: self.issues.clone(),
            plan: self.plan.clone(),
        }
    }

    pub(crate) fn node_catalog_snapshot(&self) -> NodeCatalogSnapshotV01 {
        self.node_catalog.snapshot()
    }

    pub(crate) fn preview_context(&self) -> Result<PreviewContext, Vec<RuntimeIssue>> {
        let Some(graph) = self.execution_graph() else {
            return Err(vec![RuntimeIssue::error(
                "no project loaded in runtime session",
            )]);
        };
        let Some(plan) = &self.plan else {
            return Err(vec![RuntimeIssue::error(
                "no execution plan available in runtime session",
            )]);
        };

        Ok(PreviewContext {
            graph_id: graph.id.clone(),
            graph_revision: graph.revision.clone(),
            session_revision: self.revision,
            control_revision: self.control_revision,
            graph,
            plan: plan.clone(),
            control_state: self.control_state.clone(),
        })
    }

    fn execution_graph(&self) -> Option<GraphDocument> {
        self.project
            .as_ref()
            .map(|project| lower_graph_for_execution(&project.graph))
    }

    pub fn load_project_current(
        &mut self,
        request: ProjectRequestCurrent,
    ) -> RuntimeSessionResponse {
        self.load_project_current_with_package_registry(request, None, None)
    }

    pub fn load_project_current_with_package_registry_revision(
        &mut self,
        request: ProjectRequestCurrent,
        package_registry_revision: Option<u64>,
    ) -> RuntimeSessionResponse {
        self.load_project_current_with_package_registry(request, package_registry_revision, None)
    }

    pub fn load_project_current_with_package_registry(
        &mut self,
        request: ProjectRequestCurrent,
        package_registry_revision: Option<u64>,
        package_registry: Option<PackageRegistryListResponseV01>,
    ) -> RuntimeSessionResponse {
        let package_registry = package_registry.unwrap_or_else(|| PackageRegistryListResponseV01 {
            ok: true,
            packages: Vec::new(),
            issues: Vec::new(),
        });
        let mut document = project_document_from_request_current(&request);
        let nodes_current =
            normalized_node_definitions_current(&document, request.nodes, Some(&package_registry));
        let mut load_repair_issues = repair_project_load_edges_current(&mut document);
        let view_state = runtime_owned_view_state(reconcile_view_state_with_graph_current(
            &document.graph,
            Some(document.view_state.clone()),
        ));
        document.view_state = view_state.clone();
        let request = ProjectRequestCurrent {
            document: Some(document.clone()),
            graph: document.graph.clone(),
            nodes: nodes_current.clone(),
            patch_library: document.patch_library.clone(),
            view_state: Some(view_state.clone()),
        };
        if let Err(report) = skenion_contracts::validate_project_document_v01(&document) {
            let mut issues = project_document_validation_issues_current(&document, &report);
            if let Err(runtime_issues) = validate_project_request_current(&request) {
                issues.extend(runtime_issues);
            }
            return self.response(false, issues);
        }

        let (plan, mut issues) = match build_execution_plan_request_current(&request) {
            Ok(result) => result,
            Err(issues) => return self.response(false, issues),
        };
        issues.append(&mut load_repair_issues);
        issues.extend(unresolved_object_issues_current(&document.graph));

        let graph = lower_graph_for_execution(&document.graph);
        let control_state = ControlState::from_graph(&graph);
        self.project = Some(document);
        self.nodes_current = nodes_current;
        self.plan = Some(plan);
        self.view_state = Some(view_state);
        self.control_state = control_state;
        self.view_revision = 1;
        self.control_revision = 0;
        self.issues = issues.clone();
        self.clear_history();
        self.revision += 1;
        self.package_registry_revision = package_registry_revision;
        self.package_registry = package_registry;
        self.refresh_node_catalog_cache();

        self.response(true, issues)
    }

    pub(crate) fn append_loaded_project_issues(
        &mut self,
        mut issues: Vec<RuntimeIssue>,
    ) -> Vec<RuntimeIssue> {
        self.issues.append(&mut issues);
        self.issues.clone()
    }

    pub fn apply_mutation(&mut self, mutation: RuntimeMutationRequest) -> RuntimePatchResponse {
        self.apply_mutation_with_history(mutation, RuntimeHistoryEntryKind::Apply, None)
    }

    #[cfg(test)]
    pub(crate) fn apply_patch(&mut self, patch: GraphPatch) -> RuntimePatchResponse {
        self.apply_mutation(RuntimeMutationRequest {
            graph_patch: Some(patch),
            view_patch: None,
            actor_id: None,
            client_id: None,
            description: None,
        })
    }

    pub fn reject_patch(&self, conflict: bool, issues: Vec<RuntimeIssue>) -> RuntimePatchResponse {
        self.patch_response(false, false, conflict, issues)
    }

    pub fn clear(&mut self) -> RuntimeSessionResponse {
        self.project = None;
        self.nodes_current = Vec::new();
        self.plan = None;
        self.view_state = None;
        self.control_state = ControlState::default();
        self.view_revision = 0;
        self.control_revision = 0;
        self.issues = Vec::new();
        self.clear_history();
        self.revision += 1;
        self.package_registry_revision = None;
        self.package_registry = PackageRegistryListResponseV01 {
            ok: true,
            packages: Vec::new(),
            issues: Vec::new(),
        };
        self.refresh_node_catalog_cache();
        self.response(true, Vec::new())
    }

    pub fn response(&self, ok: bool, issues: Vec<RuntimeIssue>) -> RuntimeSessionResponse {
        let snapshot = self.snapshot();
        RuntimeSessionResponse {
            ok,
            snapshot,
            issues,
        }
    }

    #[cfg(test)]
    pub(crate) fn graph(&self) -> Option<GraphDocument> {
        self.project
            .as_ref()
            .map(|project| lower_graph_for_execution(&project.graph))
    }

    pub fn project_document_current(&self) -> Option<ProjectDocumentCurrent> {
        self.project.clone()
    }

    pub(crate) fn package_registry_current(&self) -> PackageRegistryListResponseV01 {
        self.package_registry.clone()
    }

    fn refresh_node_catalog_cache(&mut self) {
        self.node_catalog
            .refresh(self.project.as_ref(), Some(&self.package_registry));
    }

    pub fn target_revision_current(&self, target: &GraphTargetRef) -> Option<String> {
        self.project
            .as_ref()
            .and_then(|project| target_graph_revision_current(project, target).ok())
    }

    pub fn view_state(&self) -> Option<ViewState> {
        self.view_state.clone()
    }

    fn apply_mutation_with_history(
        &mut self,
        mut mutation: RuntimeMutationRequest,
        kind: RuntimeHistoryEntryKind,
        subject_event_id: Option<String>,
    ) -> RuntimePatchResponse {
        let Some(current_graph) = self.project.as_ref().map(|project| project.graph.clone()) else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeIssue::error("no project loaded in runtime session")],
            );
        };

        if mutation.graph_patch.is_none() && mutation.view_patch.is_none() {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeIssue::error(
                    "runtime mutation did not include graphPatch or viewPatch",
                )],
            );
        }

        if mutation.graph_patch.is_some() {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeIssue::structured_error(
                    "project.graph-patch-unsupported",
                    "active Runtime sessions use current 0.1 ProjectDocument graph targets; graphPatch mutations are unsupported",
                    serde_json::json!({ "activeSchemaVersion": "0.1.0" }),
                )],
            );
        }

        let next_current_graph = current_graph.clone();

        if let Some(view_patch) = &mutation.view_patch
            && view_patch.base_view_revision != self.view_revision
        {
            return self.patch_response(
                false,
                false,
                true,
                vec![RuntimeIssue::error(format!(
                    "view patch baseViewRevision {} does not match session view revision {}",
                    view_patch.base_view_revision, self.view_revision
                ))],
            );
        }

        let previous_view_state = runtime_owned_view_state(
            reconcile_view_state_with_graph_current(&current_graph, self.view_state.clone()),
        );
        let mut next_view_state = reconcile_view_state_with_graph_current(
            &next_current_graph,
            Some(previous_view_state.clone()),
        );
        let view_patch = mutation
            .view_patch
            .as_ref()
            .expect("view patch should exist after no-op and active v0.1 graph patch rejection");
        let (patched_view_state, inverse_patch) = match apply_view_patch_to_view_state_current(
            &next_current_graph,
            next_view_state,
            view_patch,
        ) {
            Ok(result) => result,
            Err(issues) => {
                return self.patch_response(false, false, false, issues);
            }
        };
        next_view_state = patched_view_state;
        let inverse_view_patch = Some(inverse_patch);
        next_view_state = runtime_owned_view_state(next_view_state);
        let view_changed = previous_view_state != next_view_state;

        if !view_changed {
            return self.patch_response(true, false, false, Vec::new());
        }

        let request = ProjectRequestCurrent {
            document: self.project.clone(),
            graph: next_current_graph.clone(),
            nodes: self.nodes_current.clone(),
            patch_library: self
                .project
                .as_ref()
                .map(|project| project.patch_library.clone())
                .unwrap_or_default(),
            view_state: Some(next_view_state.clone()),
        };
        let (plan, issues) = match build_execution_plan_request_current(&request) {
            Ok(result) => result,
            Err(issues) => {
                self.plan = None;
                self.issues = issues.clone();
                return self.patch_response(false, false, false, issues);
            }
        };
        let control_state =
            ControlState::from_graph(&lower_graph_for_execution(&next_current_graph));
        let mut inverse_mutation = RuntimeMutationRequest {
            graph_patch: None,
            view_patch: inverse_view_patch,
            actor_id: mutation.actor_id.clone(),
            client_id: mutation.client_id.clone(),
            description: mutation
                .description
                .as_ref()
                .map(|description| format!("Inverse of {description}")),
        };
        normalize_mutation_base_revisions(
            &mut mutation,
            current_graph.revision.clone(),
            self.view_revision,
        );
        normalize_mutation_base_revisions(
            &mut inverse_mutation,
            next_current_graph.revision.clone(),
            self.view_revision + 1,
        );
        let history_entry = self.create_runtime_history_entry(
            kind,
            mutation.clone(),
            inverse_mutation.clone(),
            subject_event_id,
        );
        let history_stack_entry = HistoryEntry::Mutation {
            event_id: history_entry.id.clone(),
            actor_id: history_entry.actor_id.clone(),
            mutation,
            inverse_mutation,
        };

        self.plan = Some(plan);
        self.view_state = Some(next_view_state);
        if let (Some(project), Some(view_state)) = (self.project.as_mut(), self.view_state.as_ref())
        {
            project.view_state = view_state.clone();
        }
        if view_changed {
            self.view_revision += 1;
        }
        self.control_state = control_state;
        self.issues = issues.clone();
        self.revision += 1;
        self.history_entries.push(history_entry);
        if matches!(kind, RuntimeHistoryEntryKind::Apply) {
            self.undo_stack.push(history_stack_entry);
            self.redo_stack.clear();
        }

        self.patch_response(true, true, false, issues)
    }

    fn apply_project_document_update(
        &mut self,
        before: ProjectDocumentCurrent,
        after: ProjectDocumentCurrent,
        next_view_revision: u64,
        mutation: RuntimeMutationRequest,
        subject_event_id: Option<String>,
    ) -> RuntimePatchResponse {
        let before_view_revision = self.view_revision;
        let request = ProjectRequestCurrent {
            document: Some(after.clone()),
            graph: after.graph.clone(),
            nodes: self.nodes_current.clone(),
            patch_library: after.patch_library.clone(),
            view_state: Some(after.view_state.clone()),
        };
        let (plan, mut issues) = match build_execution_plan_request_current(&request) {
            Ok(result) => result,
            Err(issues) => return self.patch_response(false, false, false, issues),
        };
        issues.extend(unresolved_object_issues_current(&after.graph));
        let graph = lower_graph_for_execution(&after.graph);
        let inverse_mutation = RuntimeMutationRequest {
            graph_patch: None,
            view_patch: None,
            actor_id: mutation.actor_id.clone(),
            client_id: mutation.client_id.clone(),
            description: mutation
                .description
                .as_ref()
                .map(|description| format!("Inverse of {description}")),
        };
        let history_entry = self.create_runtime_history_entry(
            RuntimeHistoryEntryKind::Apply,
            mutation.clone(),
            inverse_mutation.clone(),
            subject_event_id,
        );
        let history_stack_entry = HistoryEntry::ProjectDocument {
            event_id: history_entry.id.clone(),
            actor_id: history_entry.actor_id.clone(),
            before: Box::new(before),
            after: Box::new(after.clone()),
            before_view_revision,
            after_view_revision: next_view_revision,
            mutation,
            inverse_mutation,
        };

        let next_view_state = after.view_state.clone();
        self.project = Some(after);
        self.refresh_node_catalog_cache();
        self.plan = Some(plan);
        self.view_state = Some(next_view_state);
        self.view_revision = next_view_revision;
        self.control_state = ControlState::from_graph(&graph);
        self.control_revision = 0;
        self.issues = issues.clone();
        self.revision += 1;
        self.history_entries.push(history_entry);
        self.undo_stack.push(history_stack_entry);
        self.redo_stack.clear();

        self.patch_response(true, true, false, issues)
    }

    fn patch_response(
        &self,
        ok: bool,
        applied: bool,
        conflict: bool,
        issues: Vec<RuntimeIssue>,
    ) -> RuntimePatchResponse {
        let snapshot = self.snapshot();
        RuntimePatchResponse {
            ok,
            applied,
            conflict,
            snapshot,
            history: self.history(),
            issues,
        }
    }

    fn apply_history_entry(
        &mut self,
        entry: HistoryEntry,
        direction: HistoryDirection,
    ) -> HistoryApplyOutcome {
        match entry {
            HistoryEntry::Mutation {
                event_id,
                mutation,
                inverse_mutation,
                ..
            } => {
                let mut mutation_to_apply = match direction {
                    HistoryDirection::Undo => inverse_mutation,
                    HistoryDirection::Redo => mutation,
                };
                self.rebase_mutation_to_current_revisions(&mut mutation_to_apply);
                let response = self.apply_mutation_with_history(
                    mutation_to_apply,
                    match direction {
                        HistoryDirection::Undo => RuntimeHistoryEntryKind::Undo,
                        HistoryDirection::Redo => RuntimeHistoryEntryKind::Redo,
                    },
                    Some(event_id),
                );
                if response.applied {
                    HistoryApplyOutcome::applied(response)
                } else {
                    HistoryApplyOutcome::rejected(response)
                }
            }
            HistoryEntry::ProjectDocument {
                event_id,
                before,
                after,
                before_view_revision,
                after_view_revision,
                mutation,
                inverse_mutation,
                ..
            } => {
                let (target_project, view_revision, mutation_to_record, inverse_to_record) =
                    match direction {
                        HistoryDirection::Undo => (
                            (*before).clone(),
                            before_view_revision,
                            inverse_mutation,
                            mutation,
                        ),
                        HistoryDirection::Redo => (
                            (*after).clone(),
                            after_view_revision,
                            mutation,
                            inverse_mutation,
                        ),
                    };
                let project = self
                    .project
                    .as_ref()
                    .map(|current| {
                        project_document_history_delta(current, &before, &after, direction)
                    })
                    .unwrap_or(target_project);
                let response = self.restore_project_document_state(
                    project,
                    view_revision,
                    match direction {
                        HistoryDirection::Undo => RuntimeHistoryEntryKind::Undo,
                        HistoryDirection::Redo => RuntimeHistoryEntryKind::Redo,
                    },
                    mutation_to_record,
                    inverse_to_record,
                    Some(event_id),
                );
                if response.applied {
                    HistoryApplyOutcome::applied(response)
                } else {
                    HistoryApplyOutcome::rejected(response)
                }
            }
        }
    }

    fn restore_project_document_state(
        &mut self,
        mut project: ProjectDocumentCurrent,
        view_revision: u64,
        mutation: RuntimeHistoryEntryKind,
        mutation_to_record: RuntimeMutationRequest,
        inverse_to_record: RuntimeMutationRequest,
        subject_event_id: Option<String>,
    ) -> RuntimePatchResponse {
        if let Some(current) = self.project.as_ref() {
            project.graph.revision = next_graph_revision(&current.graph.revision);
            project.revision = project.graph.revision.clone();
        }
        let request = ProjectRequestCurrent {
            document: Some(project.clone()),
            graph: project.graph.clone(),
            nodes: self.nodes_current.clone(),
            patch_library: project.patch_library.clone(),
            view_state: Some(project.view_state.clone()),
        };
        let (plan, mut issues) = match build_execution_plan_request_current(&request) {
            Ok(result) => result,
            Err(issues) => {
                return self.patch_response(false, false, false, issues);
            }
        };
        issues.extend(unresolved_object_issues_current(&project.graph));
        let graph = lower_graph_for_execution(&project.graph);
        let history_entry = self.create_runtime_history_entry(
            mutation,
            mutation_to_record,
            inverse_to_record,
            subject_event_id,
        );

        let next_view_state = project.view_state.clone();
        self.project = Some(project);
        self.refresh_node_catalog_cache();
        self.plan = Some(plan);
        self.view_state = Some(next_view_state);
        self.view_revision = view_revision;
        self.control_state = ControlState::from_graph(&graph);
        self.control_revision = 0;
        self.issues = issues.clone();
        self.revision += 1;
        self.history_entries.push(history_entry);

        self.patch_response(true, true, false, issues)
    }

    fn create_runtime_history_entry(
        &mut self,
        kind: RuntimeHistoryEntryKind,
        mutation: RuntimeMutationRequest,
        inverse_mutation: RuntimeMutationRequest,
        subject_event_id: Option<String>,
    ) -> RuntimeHistoryEntry {
        let sequence = self.next_event_sequence;
        self.next_event_sequence += 1;
        RuntimeHistoryEntry {
            id: format!("runtime_event_{sequence:06}"),
            sequence,
            kind,
            actor_id: mutation.actor_id.clone(),
            client_id: mutation.client_id.clone(),
            description: mutation.description.clone(),
            mutation,
            inverse_mutation,
            subject_event_id,
            created_at: created_at_now(),
        }
    }

    fn rebase_mutation_to_current_revisions(&self, mutation: &mut RuntimeMutationRequest) {
        if let (Some(project), Some(graph_patch)) =
            (self.project.as_ref(), mutation.graph_patch.as_mut())
        {
            graph_patch.base_revision = project.graph.revision.clone();
        }
        if let Some(view_patch) = mutation.view_patch.as_mut() {
            view_patch.base_view_revision = self.view_revision;
        }
    }

    fn clear_history(&mut self) {
        self.history_entries.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.next_event_sequence = 1;
    }
}

fn next_graph_revision(current: &str) -> String {
    next_graph_revision_current(current)
}

fn project_document_from_request_current(
    request: &ProjectRequestCurrent,
) -> ProjectDocumentCurrent {
    if let Some(document) = &request.document {
        return document.clone();
    }
    let graph = request.graph.clone();
    let view_state = request
        .view_state
        .clone()
        .unwrap_or_else(|| reconcile_view_state_with_graph_current(&graph, None));
    serde_json::from_value(json!({
        "schema": "skenion.project",
        "schemaVersion": "0.1.0",
        "id": graph.id.clone(),
        "documentId": "00000000-0000-0000-0000-000000000001",
        "revision": graph.revision.clone(),
        "graph": graph,
        "viewState": view_state,
        "patchLibrary": request.patch_library.clone(),
    }))
    .expect("synthesized current project document should match contract shape")
}

fn normalize_mutation_base_revisions(
    mutation: &mut RuntimeMutationRequest,
    graph_revision: String,
    view_revision: u64,
) {
    if let Some(graph_patch) = mutation.graph_patch.as_mut() {
        graph_patch.base_revision = graph_revision;
    }
    if let Some(view_patch) = mutation.view_patch.as_mut() {
        view_patch.base_view_revision = view_revision;
    }
}

fn target_graph_revision_current(
    project: &ProjectDocumentCurrent,
    target: &GraphTargetRef,
) -> Result<String, Box<RuntimeOperationIssue>> {
    Ok(target_graph_current(project, target)?.revision.clone())
}

fn target_graph_current<'a>(
    project: &'a ProjectDocumentCurrent,
    target: &GraphTargetRef,
) -> Result<&'a GraphDocumentCurrent, Box<RuntimeOperationIssue>> {
    match &target.path {
        PatchPath::Root => Ok(&project.graph),
        PatchPath::HelpWorkingCopy {
            working_copy_id, ..
        } if working_copy_id == &project.graph.id => Ok(&project.graph),
        PatchPath::HelpWorkingCopy {
            working_copy_id, ..
        } => Err(Box::new(operation_error(
            "paste.target.missing-help-working-copy",
            format!("help working copy {working_copy_id} is not loaded in this runtime session"),
            Some(target.clone()),
            None,
            Some(project.graph.revision.clone()),
            None,
            None,
        ))),
        PatchPath::ProjectPatchDefinition { patch_id } => project
            .patch_library
            .iter()
            .find(|patch| patch.id == *patch_id)
            .map(|patch| &patch.graph)
            .ok_or_else(|| {
                Box::new(operation_error(
                    "paste.target.missing-project-patch-definition",
                    format!(
                        "project patch definition {patch_id} is not loaded in this runtime session"
                    ),
                    Some(target.clone()),
                    None,
                    Some(project.graph.revision.clone()),
                    None,
                    None,
                ))
            }),
        PatchPath::PackagePatchDefinition {
            package_id,
            patch_id,
            ..
        } => Err(Box::new(operation_error(
            "paste.target.immutable-help-source",
            format!(
                "package/help source patch {package_id}/{patch_id} is immutable; paste into a project patch or help working copy instead"
            ),
            Some(target.clone()),
            None,
            Some(project.graph.revision.clone()),
            None,
            None,
        ))),
        PatchPath::EmbeddedPatchInstance { node_id, .. } => Err(Box::new(operation_error(
            "paste.target.unsupported-embedded-patch-instance",
            format!(
                "embedded patch instance owned by node {node_id} cannot be mutated by the current runtime session substrate"
            ),
            Some(target.clone()),
            None,
            Some(project.graph.revision.clone()),
            None,
            None,
        ))),
    }
}

fn apply_graph_to_project_current(
    project: &mut ProjectDocumentCurrent,
    graph: GraphDocumentCurrent,
    view_state: ViewState,
    view_changed: bool,
    path: &PatchPath,
    view_revision: u64,
) -> u64 {
    let mut next_view_revision = view_revision;
    if matches!(path, PatchPath::Root | PatchPath::HelpWorkingCopy { .. }) {
        project.graph = graph;
        project.revision = project.graph.revision.clone();
        project.view_state = runtime_owned_view_state(reconcile_view_state_with_graph_current(
            &project.graph,
            Some(view_state),
        ));
        if view_changed {
            next_view_revision += 1;
        }
    } else if let PatchPath::ProjectPatchDefinition { patch_id } = path {
        let patch = project
            .patch_library
            .iter_mut()
            .find(|patch| patch.id == *patch_id)
            .expect("project patch definition lookup was already proven");
        patch.graph = graph;
        patch.revision = patch.graph.revision.clone();
    }
    next_view_revision
}

fn graph_for_path_current(
    project: &ProjectDocumentCurrent,
    path: &PatchPath,
) -> Option<GraphDocumentCurrent> {
    match path {
        PatchPath::Root => Some(project.graph.clone()),
        PatchPath::HelpWorkingCopy {
            working_copy_id, ..
        } if working_copy_id == &project.graph.id => Some(project.graph.clone()),
        PatchPath::ProjectPatchDefinition { patch_id } => project
            .patch_library
            .iter()
            .find(|patch| patch.id == *patch_id)
            .map(|patch| patch.graph.clone()),
        PatchPath::HelpWorkingCopy { .. }
        | PatchPath::PackagePatchDefinition { .. }
        | PatchPath::EmbeddedPatchInstance { .. } => None,
    }
}

fn operation_error(
    code: impl Into<String>,
    message: impl Into<String>,
    target: Option<GraphTargetRef>,
    expected_revision: Option<String>,
    actual_revision: Option<String>,
    duplicates: Option<Vec<String>>,
    edges: Option<Vec<String>>,
) -> RuntimeOperationIssue {
    RuntimeOperationIssue {
        severity: "error".to_owned(),
        code: code.into(),
        message: message.into(),
        path: None,
        target,
        expected_revision,
        actual_revision,
        duplicates,
        nodes: None,
        edges,
        interface_policy: None,
        interface_detail: None,
    }
}

fn operation_issue_to_runtime_issue(issue: RuntimeOperationIssue) -> RuntimeIssue {
    let details = serde_json::json!({
        "path": issue.path,
        "target": issue.target,
        "expectedRevision": issue.expected_revision,
        "actualRevision": issue.actual_revision,
        "duplicates": issue.duplicates,
        "nodes": issue.nodes,
        "edges": issue.edges,
    });
    match issue.severity.as_str() {
        "warning" => RuntimeIssue::structured_warning(issue.code, issue.message, details),
        "info" => RuntimeIssue {
            severity: crate::IssueSeverity::Info,
            message: issue.message,
            code: Some(issue.code),
            details: Some(details),
        },
        _ => RuntimeIssue::structured_error(issue.code, issue.message, details),
    }
}

fn created_at_now() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("unix-ms:{millis}")
}

#[cfg(test)]
mod tests;
