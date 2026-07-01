use serde::Deserialize;
use serde_json::{Value, json};
use skenion_contracts::{NodeCatalogSnapshotV01, PackageChecksumV01};

use crate::{RuntimeSessionRecord, runtime_time::created_at_now};

use super::{
    EVENT_NODE_CATALOG_CHANGED, RUNTIME_REALTIME_SCHEMA, RUNTIME_REALTIME_SCHEMA_VERSION,
    RuntimeRealtimeEnvelope, RuntimeRealtimeIssue, state::sync_required_issue,
    wire::RuntimeRealtimeConnectionIdentity,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NodeCatalogHelloRequest {
    #[serde(default)]
    pub(crate) mode: NodeCatalogHelloMode,
    #[serde(default)]
    pub(crate) known_revision: Option<Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum NodeCatalogHelloMode {
    #[default]
    None,
    IfChanged,
    Always,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeCatalogRequestPayload {
    #[serde(default)]
    known_revision: Option<Value>,
}

pub(super) fn handle_node_catalog_request(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<RuntimeRealtimeEnvelope, RuntimeRealtimeIssue> {
    let request = serde_json::from_value::<NodeCatalogRequestPayload>(frame.payload.clone())
        .map_err(|error| {
            sync_required_issue(
                "realtime.node-catalog.invalid-payload",
                format!("invalid nodeCatalog.request payload: {error}"),
                None,
            )
        })?;
    let snapshot = node_catalog_snapshot_for_record(record);
    let (message_type, payload) =
        if catalog_revision_matches(request.known_revision.as_ref(), &snapshot.catalog_revision) {
            (
                "nodeCatalog.unchanged",
                node_catalog_unchanged_response_payload(snapshot),
            )
        } else {
            (
                "nodeCatalog.snapshot",
                node_catalog_snapshot_response_payload(snapshot),
            )
        };
    Ok(node_catalog_response(
        record,
        identity,
        &frame,
        message_type,
        payload,
    ))
}

pub(super) fn node_catalog_changed_event(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    snapshot: NodeCatalogSnapshotV01,
    sequence: u64,
    cursor: String,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: EVENT_NODE_CATALOG_CHANGED.to_owned(),
        message_id: format!("{}_node_catalog_changed_{sequence:06}", record.id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: frame.idempotency_key.clone(),
        sequence: Some(sequence),
        cursor: Some(cursor),
        created_at: Some(created_at_now()),
        payload: json!({
            "catalogRevision": snapshot.catalog_revision.clone(),
            "snapshot": snapshot,
            "replayed": false,
        }),
    }
}

pub(super) fn hello_node_catalog_payload(
    record: &RuntimeSessionRecord,
    request: Option<&NodeCatalogHelloRequest>,
) -> Value {
    let snapshot = node_catalog_snapshot_for_record(record);
    match request.map(|request| request.mode).unwrap_or_default() {
        NodeCatalogHelloMode::None => node_catalog_status_payload("notRequested", snapshot, false),
        NodeCatalogHelloMode::IfChanged
            if catalog_revision_matches(
                request.and_then(|request| request.known_revision.as_ref()),
                &snapshot.catalog_revision,
            ) =>
        {
            node_catalog_status_payload("unchanged", snapshot, false)
        }
        NodeCatalogHelloMode::IfChanged | NodeCatalogHelloMode::Always => {
            node_catalog_status_payload("included", snapshot, true)
        }
    }
}

pub(super) fn catalog_revision_matches(
    known_revision: Option<&Value>,
    catalog_revision: &PackageChecksumV01,
) -> bool {
    let Some(known_revision) = known_revision else {
        return false;
    };
    if known_revision.as_str() == Some(catalog_revision.value.as_str()) {
        return true;
    }
    serde_json::to_value(catalog_revision).expect("node catalog revision should serialize")
        == *known_revision
}

pub(crate) fn node_catalog_snapshot_for_record(
    record: &RuntimeSessionRecord,
) -> NodeCatalogSnapshotV01 {
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    node_catalog_snapshot_for_session(&session)
}

pub(super) fn node_catalog_snapshot_for_session(
    session: &crate::RuntimeSession,
) -> NodeCatalogSnapshotV01 {
    session.node_catalog_snapshot()
}

fn node_catalog_response(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    message_type: &str,
    payload: Value,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: message_type.to_owned(),
        message_id: format!("{}_node_catalog_{}", record.id, frame.message_id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: frame.idempotency_key.clone(),
        sequence: None,
        cursor: Some(record.realtime.current_cursor()),
        created_at: Some(created_at_now()),
        payload,
    }
}

fn node_catalog_snapshot_response_payload(snapshot: NodeCatalogSnapshotV01) -> Value {
    node_catalog_status_payload("included", snapshot, true)
}

fn node_catalog_unchanged_response_payload(snapshot: NodeCatalogSnapshotV01) -> Value {
    node_catalog_status_payload("unchanged", snapshot, false)
}

fn node_catalog_status_payload(
    status: &str,
    snapshot: NodeCatalogSnapshotV01,
    include_snapshot: bool,
) -> Value {
    let mut payload = json!({
        "status": status,
        "catalogRevision": snapshot.catalog_revision.clone(),
    });
    if include_snapshot && let Some(object) = payload.as_object_mut() {
        object.insert(
            "snapshot".to_owned(),
            serde_json::to_value(snapshot).expect("node catalog snapshot should serialize"),
        );
    }
    payload
}
