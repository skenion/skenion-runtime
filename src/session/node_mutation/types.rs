use skenion_contracts::InterfaceIncidentEdgePolicyV01;

use crate::{
    CanvasNodeView, GraphNodeCurrent, GraphTargetRef, NodeDefinitionCurrent, RuntimeMutationRequest,
};

pub(crate) struct ApplyObjectNodeCreateCurrentRequest {
    pub(crate) target: GraphTargetRef,
    pub(crate) node: GraphNodeCurrent,
    pub(crate) view: Option<CanvasNodeView>,
    pub(crate) definition: Option<NodeDefinitionCurrent>,
    pub(crate) mutation: RuntimeMutationRequest,
}

pub(crate) struct ApplyObjectNodeReplaceCurrentRequest {
    pub(crate) target: GraphTargetRef,
    pub(crate) node: GraphNodeCurrent,
    pub(crate) view: Option<CanvasNodeView>,
    pub(crate) definition: Option<NodeDefinitionCurrent>,
    pub(crate) interface_incident_edge_policy: Option<InterfaceIncidentEdgePolicyV01>,
    pub(crate) mutation: RuntimeMutationRequest,
}

pub(super) struct ObjectNodeCreateCurrentEdit {
    pub(super) target: GraphTargetRef,
    pub(super) node: GraphNodeCurrent,
    pub(super) view: Option<CanvasNodeView>,
    pub(super) mutation: RuntimeMutationRequest,
}

pub(super) struct ObjectNodeReplaceCurrentEdit {
    pub(super) target: GraphTargetRef,
    pub(super) node: GraphNodeCurrent,
    pub(super) view: Option<CanvasNodeView>,
    pub(super) interface_incident_edge_policy: Option<InterfaceIncidentEdgePolicyV01>,
    pub(super) mutation: RuntimeMutationRequest,
}
