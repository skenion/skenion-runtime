use std::time::Duration;

pub const RUNTIME_REALTIME_SCHEMA: &str = "skenion.runtime.realtime";
pub const RUNTIME_REALTIME_SCHEMA_VERSION: &str = "0.1.0";
pub const RUNTIME_REALTIME_REPLAY_LIMIT: usize = 256;

pub(super) const RUNTIME_REALTIME_PRESENCE_LIMIT_MULTIPLIER: usize = 2;
pub(super) const RUNTIME_REALTIME_RESUME_TOKEN_TTL: Duration = Duration::from_secs(5 * 60);
pub(super) const RUNTIME_REALTIME_RESUME_TOKEN_BYTES: usize = 32;

pub(super) const FRAME_SESSION_HELLO: &str = "session.hello";
pub(super) const FRAME_SELECTION_UPDATE: &str = "selection.update";
pub(super) const FRAME_GRAPH_COMMAND: &str = "graph.command";
pub(super) const FRAME_NODE_INPUT: &str = "node.input";
pub(super) const FRAME_NODE_CATALOG_REQUEST: &str = "nodeCatalog.request";

pub(super) const EVENT_SELECTION_UPDATED: &str = "selection.updated";
pub(super) const EVENT_CONTROL_EMITTED: &str = "control.emitted";
pub(super) const EVENT_GRAPH_APPLIED: &str = "graph.applied";
pub(super) const EVENT_NODE_CATALOG_CHANGED: &str = "nodeCatalog.changed";
pub(super) const EVENT_COMMAND_ACK: &str = "command.ack";
pub(super) const EVENT_RUNTIME_ISSUE: &str = "runtime.issue";

pub(super) const GRAPH_KIND_VIEW_PATCH: &str = "view.patch";
pub(super) const GRAPH_KIND_CHANGE_SET: &str = "graph.changeSet";
pub(super) const GRAPH_KIND_PASTE_FRAGMENT: &str = "graph.pasteFragment";
pub(super) const GRAPH_KIND_NODE_RESOLVE: &str = "node.resolve";
pub(super) const GRAPH_KIND_NODE_CREATE: &str = "node.create";
pub(super) const GRAPH_KIND_NODE_REPLACE: &str = "node.replace";
pub(super) const GRAPH_KIND_NODE_DELETE: &str = "node.delete";
pub(super) const GRAPH_KIND_NODE_UPDATE: &str = "node.update";
pub(super) const GRAPH_KIND_HISTORY_UNDO: &str = "history.undo";
pub(super) const GRAPH_KIND_HISTORY_REDO: &str = "history.redo";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GraphCommandKind {
    ViewPatch,
    ChangeSet,
    PasteFragment,
    NodeResolve,
    NodeCreate,
    NodeReplace,
    NodeDelete,
    NodeUpdate,
    HistoryUndo,
    HistoryRedo,
}

impl GraphCommandKind {
    pub(super) fn parse(kind: &str) -> Option<Self> {
        match kind {
            GRAPH_KIND_VIEW_PATCH => Some(Self::ViewPatch),
            GRAPH_KIND_CHANGE_SET => Some(Self::ChangeSet),
            GRAPH_KIND_PASTE_FRAGMENT => Some(Self::PasteFragment),
            GRAPH_KIND_NODE_RESOLVE => Some(Self::NodeResolve),
            GRAPH_KIND_NODE_CREATE => Some(Self::NodeCreate),
            GRAPH_KIND_NODE_REPLACE => Some(Self::NodeReplace),
            GRAPH_KIND_NODE_DELETE => Some(Self::NodeDelete),
            GRAPH_KIND_NODE_UPDATE => Some(Self::NodeUpdate),
            GRAPH_KIND_HISTORY_UNDO => Some(Self::HistoryUndo),
            GRAPH_KIND_HISTORY_REDO => Some(Self::HistoryRedo),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::ViewPatch => GRAPH_KIND_VIEW_PATCH,
            Self::ChangeSet => GRAPH_KIND_CHANGE_SET,
            Self::PasteFragment => GRAPH_KIND_PASTE_FRAGMENT,
            Self::NodeResolve => GRAPH_KIND_NODE_RESOLVE,
            Self::NodeCreate => GRAPH_KIND_NODE_CREATE,
            Self::NodeReplace => GRAPH_KIND_NODE_REPLACE,
            Self::NodeDelete => GRAPH_KIND_NODE_DELETE,
            Self::NodeUpdate => GRAPH_KIND_NODE_UPDATE,
            Self::HistoryUndo => GRAPH_KIND_HISTORY_UNDO,
            Self::HistoryRedo => GRAPH_KIND_HISTORY_REDO,
        }
    }
}

const GRAPH_COMMAND_SUPPORTED_KINDS: &[GraphCommandKind] = &[
    GraphCommandKind::ViewPatch,
    GraphCommandKind::ChangeSet,
    GraphCommandKind::PasteFragment,
    GraphCommandKind::NodeResolve,
    GraphCommandKind::NodeCreate,
    GraphCommandKind::NodeReplace,
    GraphCommandKind::NodeDelete,
    GraphCommandKind::NodeUpdate,
    GraphCommandKind::HistoryUndo,
    GraphCommandKind::HistoryRedo,
];

pub(super) fn graph_command_supported_kind_names() -> Vec<&'static str> {
    GRAPH_COMMAND_SUPPORTED_KINDS
        .iter()
        .map(|kind| kind.as_str())
        .collect()
}
