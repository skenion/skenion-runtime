use std::{fs, net::SocketAddr, path::Path, time::Duration};

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, header},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use skenion_contracts::{NodeCatalogSnapshotV01, validate_node_catalog_snapshot_v01};
use skenion_runtime::{RuntimeServerState, runtime_router_with_state};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Error as TungsteniteError, Message},
};
use tower::ServiceExt;

type TestSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct TestRuntime {
    addr: SocketAddr,
    handle: JoinHandle<()>,
    state: RuntimeServerState,
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn spawn_runtime() -> TestRuntime {
    spawn_runtime_with_state(RuntimeServerState::default()).await
}

async fn spawn_runtime_with_state(state: RuntimeServerState) -> TestRuntime {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test listener binds");
    let addr = listener.local_addr().expect("test listener has local addr");
    let app = runtime_router_with_state(state.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("runtime serves");
    });
    TestRuntime {
        addr,
        handle,
        state,
    }
}

async fn spawn_loaded_runtime() -> TestRuntime {
    let state = RuntimeServerState::default();
    let app = runtime_router_with_state(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v0/sessions/default/load")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    session_load_request(sample_project_document_current()).to_string(),
                ))
                .expect("load request builds"),
        )
        .await
        .expect("load request succeeds");
    assert!(response.status().is_success());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("load response body should be readable");
    let load_response: Value =
        serde_json::from_slice(&body).expect("load response body should be JSON");
    assert_eq!(load_response["ok"], true, "load failed: {load_response}");
    spawn_runtime_with_state(state).await
}

async fn get_json_from_state(state: &RuntimeServerState, path: &str) -> Value {
    let response = runtime_router_with_state(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("request succeeds");
    assert!(
        response.status().is_success(),
        "{path} should return success, got {}",
        response.status()
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response body should be JSON")
}

async fn connect_session(runtime: &TestRuntime, session_id: &str) -> TestSocket {
    let url = format!("ws://{}/v0/sessions/{session_id}", runtime.addr);
    let (socket, _) = connect_async(url).await.expect("websocket connects");
    socket
}

async fn send_json(socket: &mut TestSocket, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("websocket send succeeds");
}

async fn next_json(socket: &mut TestSocket) -> Value {
    loop {
        let message = timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("websocket frame arrives")
            .expect("websocket remains open")
            .expect("websocket frame succeeds");
        match message {
            Message::Text(text) => {
                return serde_json::from_str(text.as_ref()).expect("frame is JSON");
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(close) => panic!("websocket closed unexpectedly: {close:?}"),
            Message::Binary(_) | Message::Frame(_) => panic!("unexpected websocket frame"),
        }
    }
}

async fn next_type(socket: &mut TestSocket, message_type: &str) -> Value {
    loop {
        let frame = next_json(socket).await;
        if frame["type"] == message_type {
            return frame;
        }
    }
}

async fn attach(socket: &mut TestSocket, message_id: &str, last_cursor: Option<&str>) -> Value {
    attach_with_resume(socket, message_id, last_cursor, None).await
}

async fn attach_with_node_catalog(
    socket: &mut TestSocket,
    message_id: &str,
    mode: &str,
    known_revision: Option<Value>,
) -> Value {
    let mut node_catalog = json!({ "mode": mode });
    if let Some(known_revision) = known_revision {
        node_catalog["knownRevision"] = known_revision;
    }
    attach_with_payload(
        socket,
        message_id,
        json!({
            "clientId": "client-hint",
            "windowId": "window-hint",
            "hints": { "label": "test" },
            "nodeCatalog": node_catalog,
        }),
    )
    .await
}

async fn attach_with_resume(
    socket: &mut TestSocket,
    message_id: &str,
    last_cursor: Option<&str>,
    resume_token: Option<&str>,
) -> Value {
    let mut payload = json!({
        "clientId": "client-hint",
        "windowId": "window-hint",
        "hints": { "label": "test" }
    });
    if let Some(last_cursor) = last_cursor {
        payload["lastCursor"] = Value::String(last_cursor.to_owned());
    }
    if let Some(resume_token) = resume_token {
        payload["resumeToken"] = Value::String(resume_token.to_owned());
    }
    attach_with_payload(socket, message_id, payload).await
}

async fn attach_with_payload(socket: &mut TestSocket, message_id: &str, payload: Value) -> Value {
    send_json(
        socket,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "session.hello",
            "messageId": message_id,
            "sessionId": "default",
            "clientId": "client-hint",
            "windowId": "window-hint",
            "payload": payload
        }),
    )
    .await;
    next_json(socket).await
}

async fn send_node_catalog_request(
    socket: &mut TestSocket,
    message_id: &str,
    known_revision: Option<Value>,
) {
    let mut payload = json!({});
    if let Some(known_revision) = known_revision {
        payload["knownRevision"] = known_revision;
    }
    send_json(
        socket,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "nodeCatalog.request",
            "messageId": message_id,
            "sessionId": "default",
            "commandId": message_id,
            "correlationId": message_id,
            "payload": payload
        }),
    )
    .await;
}

async fn send_node_catalog_request_payload(
    socket: &mut TestSocket,
    message_id: &str,
    payload: Value,
) {
    send_json(
        socket,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "nodeCatalog.request",
            "messageId": message_id,
            "sessionId": "default",
            "commandId": message_id,
            "correlationId": message_id,
            "payload": payload
        }),
    )
    .await;
}

async fn send_presence(socket: &mut TestSocket, message_id: &str, idempotency_key: &str) {
    send_presence_payload(
        socket,
        message_id,
        idempotency_key,
        json!({
            "state": "active",
            "selection": { "nodeIds": ["value_1"] }
        }),
    )
    .await;
}

async fn send_presence_payload(
    socket: &mut TestSocket,
    message_id: &str,
    idempotency_key: &str,
    presence: Value,
) {
    send_json(
        socket,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "presence.update",
            "messageId": message_id,
            "sessionId": "default",
            "commandId": message_id,
            "correlationId": message_id,
            "idempotencyKey": idempotency_key,
            "payload": {
                "ttlMs": 30000,
                "presence": presence
            }
        }),
    )
    .await;
}

async fn send_control_command(
    socket: &mut TestSocket,
    message_id: &str,
    idempotency_key: &str,
    node_id: &str,
    port_id: &str,
    message: Value,
) {
    send_graph_command(
        socket,
        message_id,
        idempotency_key,
        node_input_payload(node_id, port_id, message),
    )
    .await;
}

async fn send_graph_command(
    socket: &mut TestSocket,
    message_id: &str,
    idempotency_key: &str,
    payload: Value,
) {
    send_json(
        socket,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "graph.command",
            "messageId": message_id,
            "sessionId": "default",
            "commandId": message_id,
            "correlationId": message_id,
            "idempotencyKey": idempotency_key,
            "payload": payload
        }),
    )
    .await;
}

fn root_target(base_revision: &str) -> Value {
    json!({
        "path": { "kind": "root" },
        "baseRevision": base_revision
    })
}

fn view_patch_payload(base_view_revision: u64, x: f64, y: f64) -> Value {
    json!({
        "kind": "view.patch",
        "baseSessionRevision": 1,
        "baseGraphRevision": "1",
        "baseViewRevision": base_view_revision,
        "target": root_target("1"),
        "surfacePath": { "surface": "canvas", "nodeId": "value_1" },
        "viewPatch": {
            "baseViewRevision": base_view_revision,
            "ops": [
                {
                    "op": "moveNodeView",
                    "nodeId": "value_1",
                    "from": { "x": 96.0, "y": 96.0 },
                    "to": { "x": x, "y": y }
                }
            ]
        }
    })
}

fn graph_node_add_payload(base_revision: &str, node_id: &str) -> Value {
    json!({
        "kind": "graph.changeSet",
        "baseSessionRevision": 1,
        "baseGraphRevision": base_revision,
        "target": root_target(base_revision),
        "surfacePath": { "surface": "graph", "path": { "kind": "root" } },
        "changes": [
            {
                "op": "node.add",
                "changeId": format!("add-{node_id}"),
                "node": {
                    "id": node_id,
                    "kind": "object.core.float",
                    "kindVersion": "0.1.0",
                    "params": {},
                    "ports": value_f32_ports_current_json()
                },
                "view": { "x": 360.0, "y": 180.0 }
            }
        ]
    })
}

fn paste_fragment_payload(base_revision: &str, node_id: &str) -> Value {
    json!({
        "kind": "graph.pasteFragment",
        "baseGraphRevision": base_revision,
        "request": {
            "target": root_target(base_revision),
            "fragment": {
                "schema": "skenion.graph.fragment",
                "schemaVersion": "0.1.0",
                "nodes": [
                    {
                        "id": node_id,
                        "kind": "object.core.float",
                        "kindVersion": "0.1.0",
                        "params": {},
                        "ports": value_f32_ports_current_json()
                    }
                ],
                "edges": []
            },
            "options": {
                "idConflictPolicy": "remap"
            }
        }
    })
}

fn history_payload(kind: &str) -> Value {
    json!({ "kind": kind })
}

fn graph_edge_disconnect_payload(base_revision: &str, edge_id: &str) -> Value {
    json!({
        "kind": "graph.changeSet",
        "baseSessionRevision": 1,
        "baseGraphRevision": base_revision,
        "target": root_target(base_revision),
        "surfacePath": { "surface": "graph", "path": { "kind": "root" } },
        "changes": [
            {
                "op": "edge.disconnect",
                "changeId": format!("disconnect-{edge_id}"),
                "edgeId": edge_id
            }
        ]
    })
}

fn project_patch_target(patch_id: &str, base_revision: &str) -> Value {
    json!({
        "path": { "kind": "project-patch-definition", "patchId": patch_id },
        "baseRevision": base_revision
    })
}

fn project_patch_interface_node_add_payload(patch_id: &str, base_revision: &str) -> Value {
    json!({
        "kind": "graph.changeSet",
        "baseSessionRevision": 1,
        "baseGraphRevision": base_revision,
        "target": project_patch_target(patch_id, base_revision),
        "surfacePath": {
            "surface": "graph",
            "path": { "kind": "project-patch-definition", "patchId": patch_id }
        },
        "changes": [
            {
                "op": "node.add",
                "changeId": format!("add-{patch_id}-inlet"),
                "node": {
                    "id": "patch_in",
                    "kind": "object.core.inlet",
                    "kindVersion": "0.1.0",
                    "params": { "portId": "value", "label": "Value" },
                    "ports": [
                        { "id": "out", "direction": "output", "type": "value.core.float32", "rate": "control" }
                    ]
                }
            }
        ]
    })
}

fn project_patch_internal_node_add_payload(patch_id: &str, base_revision: &str) -> Value {
    json!({
        "kind": "graph.changeSet",
        "baseSessionRevision": 1,
        "baseGraphRevision": base_revision,
        "target": project_patch_target(patch_id, base_revision),
        "surfacePath": {
            "surface": "graph",
            "path": { "kind": "project-patch-definition", "patchId": patch_id }
        },
        "changes": [
            {
                "op": "node.add",
                "changeId": format!("add-{patch_id}-internal-float"),
                "node": {
                    "id": "internal_float",
                    "kind": "object.core.float",
                    "kindVersion": "0.1.0",
                    "params": { "value": 42.0 },
                    "ports": value_f32_ports_current_json()
                }
            }
        ]
    })
}

fn node_create_payload(base_revision: &str, requested_node_id: &str, object_text: &str) -> Value {
    node_create_payload_with_params(base_revision, requested_node_id, object_text, None)
}

fn node_create_payload_without_requested_id(base_revision: &str, object_text: &str) -> Value {
    let mut payload = node_create_payload(base_revision, "removed-by-test", object_text);
    payload
        .as_object_mut()
        .expect("node.create payload should be an object")
        .remove("requestedNodeId");
    payload
}

fn node_create_payload_with_params(
    base_revision: &str,
    requested_node_id: &str,
    object_text: &str,
    params: Option<Value>,
) -> Value {
    let mut payload = json!({
        "kind": "node.create",
        "baseSessionRevision": 1,
        "baseGraphRevision": base_revision,
        "target": root_target(base_revision),
        "surfacePath": { "surface": "graph", "path": { "kind": "root" } },
        "requestedNodeId": requested_node_id,
        "objectText": object_text,
        "view": { "x": 420.0, "y": 144.0 }
    });
    if let Some(params) = params {
        payload
            .as_object_mut()
            .expect("node.create payload should be an object")
            .insert("params".to_owned(), params);
    }
    payload
}

fn node_create_project_patch_payload(
    patch_id: &str,
    base_revision: &str,
    requested_node_id: &str,
    object_text: &str,
) -> Value {
    let mut payload = node_create_payload(base_revision, requested_node_id, object_text);
    let object = payload
        .as_object_mut()
        .expect("node.create payload should be an object");
    object.insert(
        "target".to_owned(),
        project_patch_target(patch_id, base_revision),
    );
    object.insert(
        "surfacePath".to_owned(),
        json!({
            "surface": "graph",
            "path": { "kind": "project-patch-definition", "patchId": patch_id }
        }),
    );
    payload
}

fn node_resolve_payload(base_revision: &str, object_text: &str) -> Value {
    json!({
        "kind": "node.resolve",
        "baseSessionRevision": 1,
        "baseGraphRevision": base_revision,
        "target": root_target(base_revision),
        "surfacePath": { "surface": "objectText" },
        "objectText": object_text
    })
}

fn node_resolve_payload_with_target(
    base_revision: &str,
    object_text: &str,
    target: Value,
) -> Value {
    let mut payload = node_resolve_payload(base_revision, object_text);
    payload
        .as_object_mut()
        .expect("node.resolve payload should be an object")
        .insert("target".to_owned(), target);
    payload
}

fn node_replace_payload_with_params(
    base_revision: &str,
    node_id: &str,
    object_text: &str,
    params: Option<Value>,
) -> Value {
    let mut payload = json!({
        "kind": "node.replace",
        "baseSessionRevision": 1,
        "baseGraphRevision": base_revision,
        "target": root_target(base_revision),
        "surfacePath": { "surface": "graph", "path": { "kind": "root" }, "nodeId": node_id },
        "nodeId": node_id,
        "objectText": object_text,
        "interfaceIncidentEdgePolicy": "drop"
    });
    if let Some(params) = params {
        payload
            .as_object_mut()
            .expect("node.replace payload should be an object")
            .insert("params".to_owned(), params);
    }
    payload
}

fn node_replace_payload_with_policy(
    base_revision: &str,
    node_id: &str,
    object_text: &str,
    policy: &str,
) -> Value {
    let mut payload = node_replace_payload_with_params(base_revision, node_id, object_text, None);
    payload
        .as_object_mut()
        .expect("node.replace payload should be an object")
        .insert("interfaceIncidentEdgePolicy".to_owned(), json!(policy));
    payload
}

fn node_delete_payload(base_revision: &str, node_id: &str) -> Value {
    json!({
        "kind": "node.delete",
        "baseSessionRevision": 1,
        "baseGraphRevision": base_revision,
        "target": root_target(base_revision),
        "surfacePath": { "surface": "graph", "path": { "kind": "root" }, "nodeId": node_id },
        "nodeId": node_id
    })
}

fn node_update_payload(base_revision: &str, node_id: &str, params: Value) -> Value {
    json!({
        "kind": "node.update",
        "baseSessionRevision": 1,
        "baseGraphRevision": base_revision,
        "target": root_target(base_revision),
        "surfacePath": { "surface": "graph", "path": { "kind": "root" }, "nodeId": node_id },
        "nodeId": node_id,
        "params": params
    })
}

fn node_input_payload(node_id: &str, port_id: &str, message: Value) -> Value {
    json!({
        "kind": "node.input",
        "baseSessionRevision": 1,
        "surfacePath": { "surface": "graph", "path": { "kind": "root" }, "nodeId": node_id },
        "nodeId": node_id,
        "portId": port_id,
        "message": message
    })
}

fn legacy_object_command_payload(kind: &str) -> Value {
    json!({
        "kind": kind,
        "baseSessionRevision": 1,
        "baseGraphRevision": "1",
        "target": root_target("1"),
        "objectText": "osc~ 220",
        "nodeId": "value_1",
        "requestedNodeId": "legacy_1"
    })
}

fn loaded_project_json(runtime: &TestRuntime) -> Value {
    let record = runtime.state.sessions.get_or_create("default");
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    serde_json::to_value(
        session
            .project_document_current()
            .expect("test runtime should have a loaded project"),
    )
    .expect("project document should serialize")
}

fn loaded_control_values_json(runtime: &TestRuntime) -> Value {
    let record = runtime.state.sessions.get_or_create("default");
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    serde_json::to_value(session.control_state_response().values)
        .expect("control state values should serialize")
}

fn graph_node<'a>(project: &'a Value, node_id: &str) -> &'a Value {
    project["graph"]["nodes"]
        .as_array()
        .expect("project graph should contain nodes")
        .iter()
        .find(|node| node["id"] == node_id)
        .unwrap_or_else(|| panic!("node {node_id} should exist"))
}

fn graph_node_option<'a>(project: &'a Value, node_id: &str) -> Option<&'a Value> {
    project["graph"]["nodes"]
        .as_array()
        .expect("project graph should contain nodes")
        .iter()
        .find(|node| node["id"] == node_id)
}

fn graph_edge_ids(project: &Value) -> Vec<String> {
    project["graph"]["edges"]
        .as_array()
        .expect("project graph should contain edges")
        .iter()
        .map(|edge| edge["id"].as_str().expect("edge should have id").to_owned())
        .collect()
}

fn assert_source_tree_lacks(path: impl AsRef<Path>, needle: &str) {
    let path = path.as_ref();
    for entry in fs::read_dir(path).unwrap_or_else(|error| {
        panic!("source path {} should be readable: {error}", path.display())
    }) {
        let entry = entry.expect("source entry should be readable");
        let entry_path = entry.path();
        if entry_path.is_dir() {
            assert_source_tree_lacks(&entry_path, needle);
            continue;
        }
        if entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("rs")
        {
            continue;
        }
        let text = fs::read_to_string(&entry_path).unwrap_or_else(|error| {
            panic!(
                "source file {} should be readable: {error}",
                entry_path.display()
            )
        });
        assert!(
            !text.contains(needle),
            "{} must not import or use contract command-wire types containing {needle}",
            entry_path.display()
        );
    }
}

fn bang_message() -> Value {
    json!({ "selector": "bang", "atoms": [] })
}

fn float_message(value: f64) -> Value {
    json!({
        "selector": "float",
        "atoms": [{ "type": "float", "representation": "f32", "value": value }]
    })
}

fn set_float_message(value: f64) -> Value {
    json!({
        "selector": "set",
        "atoms": [{ "type": "float", "representation": "f32", "value": value }]
    })
}

#[tokio::test]
async fn plain_realtime_get_returns_upgrade_required_without_creating_session() {
    let state = RuntimeServerState::default();
    let app = runtime_router_with_state(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v0/sessions/probe")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("plain GET returns response");

    assert_eq!(response.status(), 426);
    assert!(state.sessions.get_existing("probe").is_none());
}

#[tokio::test]
async fn node_catalog_get_for_unknown_session_returns_not_found_without_creating_session() {
    let state = RuntimeServerState::default();
    let app = runtime_router_with_state(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v0/sessions/probe/node-catalog")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("node catalog GET returns response");

    assert_eq!(response.status(), 404);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    let body: Value = serde_json::from_slice(&body).expect("response body should be JSON");
    assert_eq!(body["ok"], false);
    assert_eq!(body["sessionId"], "probe");
    assert_eq!(body["diagnostic"]["code"], "runtime.session-not-found");
    assert!(state.sessions.get_existing("probe").is_none());
}

#[tokio::test]
async fn websocket_attach_returns_server_issued_identity_snapshot_and_cursor() {
    let app = runtime_router_with_state(RuntimeServerState::default());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v0/sessions/default")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("plain GET returns response");
    assert_eq!(response.status(), 426);

    let runtime = spawn_runtime().await;
    let mut socket = connect_session(&runtime, "default").await;
    let attached = attach(&mut socket, "hello-1", None).await;

    assert_eq!(attached["type"], "session.attached");
    assert_eq!(attached["schema"], "skenion.runtime.realtime");
    assert_eq!(attached["sessionId"], "default");
    assert_ne!(attached["clientId"], "client-hint");
    assert_ne!(attached["windowId"], "window-hint");
    assert!(attached["connectionId"].as_str().is_some());
    assert!(attached["payload"]["resumeToken"].as_str().is_some());
    assert!(attached["payload"]["currentRevisions"]["sessionRevision"].is_u64());
    assert!(attached["payload"]["snapshot"].is_object());
    assert!(attached["payload"]["globalCursor"].as_str().is_some());
}

#[tokio::test]
async fn websocket_rejects_invalid_frames_session_mismatch_and_pre_attach_commands() {
    let runtime = spawn_runtime().await;
    let mut socket = connect_session(&runtime, "default").await;

    socket
        .send(Message::Text("{".into()))
        .await
        .expect("invalid JSON frame sends");
    let invalid_json = next_type(&mut socket, "runtime.error").await;
    assert_eq!(
        invalid_json["payload"]["diagnostic"]["code"],
        "realtime.frame.invalid-json"
    );

    socket
        .send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .expect("binary frame sends");
    let binary = next_type(&mut socket, "runtime.error").await;
    assert_eq!(
        binary["payload"]["diagnostic"]["code"],
        "realtime.frame.binary-unsupported"
    );

    send_json(
        &mut socket,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "session.hello",
            "messageId": "hello-wrong-session",
            "sessionId": "other",
            "payload": {}
        }),
    )
    .await;
    let mismatch = next_type(&mut socket, "runtime.error").await;
    assert_eq!(
        mismatch["payload"]["diagnostic"]["code"],
        "realtime.session.mismatch"
    );
    assert_eq!(
        mismatch["payload"]["diagnostic"]["details"]["expectedSessionId"],
        "default"
    );
    assert_eq!(
        mismatch["payload"]["diagnostic"]["details"]["actualSessionId"],
        "other"
    );

    for (message_type, payload) in [
        (
            "presence.update",
            json!({
                "ttlMs": 30000,
                "presence": { "state": "active" }
            }),
        ),
        (
            "control.command",
            json!({
                "nodeId": "value_1",
                "portId": "in",
                "message": bang_message()
            }),
        ),
        ("graph.command", view_patch_payload(1, 120.0, 120.0)),
        ("nodeCatalog.request", json!({})),
    ] {
        send_json(
            &mut socket,
            json!({
                "schema": "skenion.runtime.realtime",
                "schemaVersion": "0.1.0",
                "type": message_type,
                "messageId": format!("pre-attach-{message_type}"),
                "sessionId": "default",
                "commandId": format!("pre-attach-{message_type}"),
                "correlationId": format!("pre-attach-{message_type}"),
                "idempotencyKey": format!("pre-attach-key-{message_type}"),
                "payload": payload
            }),
        )
        .await;
        let error = next_type(&mut socket, "runtime.error").await;
        assert_eq!(
            error["payload"]["diagnostic"]["code"], "realtime.session.not-attached",
            "{message_type}"
        );
    }
}

#[tokio::test]
async fn realtime_node_catalog_request_rejects_malformed_payload() {
    let runtime = spawn_loaded_runtime().await;
    let mut socket = connect_session(&runtime, "default").await;
    let attached = attach(&mut socket, "hello-1", None).await;

    send_node_catalog_request_payload(
        &mut socket,
        "catalog-request-invalid",
        Value::String("not-an-object".to_owned()),
    )
    .await;
    let error = next_type(&mut socket, "runtime.error").await;

    assert_eq!(attached["type"], "session.attached");
    assert_eq!(
        error["payload"]["diagnostic"]["code"],
        "realtime.node-catalog.invalid-payload"
    );
    assert_eq!(error["connectionId"], attached["connectionId"]);
    assert_eq!(error["clientId"], attached["clientId"]);
    assert_eq!(error["windowId"], attached["windowId"]);
}

#[tokio::test]
async fn realtime_attached_commands_reject_missing_idempotency_invalid_payloads_and_node_input_fields()
 {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_json(
        &mut client_a,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "presence.update",
            "messageId": "presence-missing-idempotency",
            "sessionId": "default",
            "payload": {
                "ttlMs": 30000,
                "presence": { "state": "active" }
            }
        }),
    )
    .await;
    let missing_presence_key = next_type(&mut client_a, "runtime.error").await;

    send_json(
        &mut client_a,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "control.command",
            "messageId": "control-invalid-payload",
            "sessionId": "default",
            "idempotencyKey": "control-invalid-payload-key",
            "payload": "not-an-object"
        }),
    )
    .await;
    let invalid_control_payload = next_type(&mut client_a, "runtime.error").await;

    send_json(
        &mut client_a,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "graph.command",
            "messageId": "graph-invalid-payload",
            "sessionId": "default",
            "idempotencyKey": "graph-invalid-payload-key",
            "payload": "not-an-object"
        }),
    )
    .await;
    let invalid_graph_payload = next_type(&mut client_a, "runtime.error").await;

    for (name, payload, expected_code) in [
        (
            "missing-node",
            json!({
                "kind": "node.input",
                "baseSessionRevision": 1,
                "portId": "in",
                "message": bang_message()
            }),
            "graph.command.node-id-required",
        ),
        (
            "missing-port",
            json!({
                "kind": "node.input",
                "baseSessionRevision": 1,
                "nodeId": "value_1",
                "message": bang_message()
            }),
            "graph.command.port-id-required",
        ),
        (
            "missing-message",
            json!({
                "kind": "node.input",
                "baseSessionRevision": 1,
                "nodeId": "value_1",
                "portId": "in"
            }),
            "graph.command.message-required",
        ),
    ] {
        send_graph_command(
            &mut client_a,
            &format!("node-input-{name}"),
            &format!("node-input-{name}-key"),
            payload,
        )
        .await;
        let ack = next_type(&mut client_a, "graph.ack").await;
        assert_eq!(ack["payload"]["status"], "rejected", "{name}");
        assert_eq!(
            ack["payload"]["diagnostics"][0]["code"], expected_code,
            "{name}"
        );
    }

    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_eq!(
        missing_presence_key["payload"]["diagnostic"]["code"],
        "realtime.command.idempotency-key-required"
    );
    assert_eq!(
        invalid_control_payload["payload"]["diagnostic"]["code"],
        "realtime.control-command.disabled"
    );
    assert_eq!(
        invalid_graph_payload["payload"]["diagnostic"]["code"],
        "realtime.graph.invalid-payload"
    );
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn realtime_control_bang_broadcasts_control_emitted_to_attached_clients() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_control_command(
        &mut client_a,
        "control-bang-1",
        "control-bang-key",
        "value_1",
        "in",
        bang_message(),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "control.emitted").await;

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(ack["payload"]["commandId"], "control-bang-1");
    assert_eq!(ack["payload"]["correlationId"], "control-bang-1");
    assert_eq!(ack["payload"]["idempotencyKey"], "control-bang-key");
    assert_eq!(ack["payload"]["kind"], "node.input");
    assert!(
        ack["payload"]["node"]["input"]["controlRevision"]
            .as_u64()
            .is_some()
    );
    assert_eq!(broadcast["clientId"], attached_a["clientId"]);
    assert_eq!(broadcast["windowId"], attached_a["windowId"]);
    assert_eq!(broadcast["payload"]["emitted"][0]["nodeId"], "value_1");
    assert_eq!(broadcast["payload"]["emitted"][0]["portId"], "value");
    assert_eq!(
        broadcast["payload"]["emitted"][0]["message"],
        float_message(0.0)
    );
}

#[tokio::test]
async fn realtime_control_float_applies_through_session_and_broadcasts_result() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_control_command(
        &mut client_a,
        "control-float-1",
        "control-float-key",
        "value_1",
        "in",
        float_message(12.0),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "control.emitted").await;

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(ack["payload"]["node"]["input"]["changed"], true);
    assert_eq!(ack["payload"]["node"]["input"]["controlRevision"], 1);
    assert_eq!(broadcast["payload"]["controlRevision"], 1);
    assert_eq!(broadcast["payload"]["emitted"][0]["nodeId"], "value_1");
    assert_eq!(
        broadcast["payload"]["emitted"][0]["message"],
        float_message(12.0)
    );
    assert_eq!(
        broadcast["payload"]["values"]["value_1"],
        json!({ "type": "float", "representation": "f32", "value": 12.0 })
    );
    assert_eq!(
        broadcast["payload"]["values"]["target_1"],
        json!({ "type": "float", "representation": "f32", "value": 12.0 })
    );
}

#[tokio::test]
async fn realtime_control_invalid_command_returns_rejected_ack_without_success_broadcast() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_control_command(
        &mut client_a,
        "control-invalid-1",
        "control-invalid-key",
        "missing",
        "in",
        float_message(12.0),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let no_success_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_eq!(ack["payload"]["status"], "rejected");
    assert_eq!(ack["payload"]["accepted"], false);
    assert_eq!(ack["payload"]["diagnostics"][0]["severity"], "error");
    assert!(
        ack["payload"]["diagnostics"][0]["message"]
            .as_str()
            .expect("diagnostic message")
            .contains("does not exist")
    );
    assert!(no_success_broadcast.is_err());
}

#[tokio::test]
async fn realtime_control_duplicate_idempotency_key_replays_ack_without_second_apply_or_broadcast()
{
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_control_command(
        &mut client_a,
        "control-once",
        "control-dedupe-key",
        "value_1",
        "in",
        float_message(12.0),
    )
    .await;
    let first_ack = next_type(&mut client_a, "graph.ack").await;
    let client_a_echo = next_type(&mut client_a, "control.emitted").await;
    let first_broadcast = next_type(&mut client_b, "control.emitted").await;

    send_control_command(
        &mut client_a,
        "control-duplicate",
        "control-dedupe-key",
        "value_1",
        "in",
        float_message(24.0),
    )
    .await;
    let duplicate_ack = next_type(&mut client_a, "graph.ack").await;
    let duplicate_local_result = next_type(&mut client_a, "control.emitted").await;
    let no_second_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_ne!(duplicate_ack["messageId"], first_ack["messageId"]);
    assert_eq!(duplicate_ack["payload"]["accepted"], true);
    assert_eq!(duplicate_ack["payload"]["cached"], true);
    assert_eq!(
        duplicate_ack["payload"]["eventCursor"],
        first_ack["payload"]["eventCursor"]
    );
    assert_eq!(
        duplicate_ack["payload"]["node"]["input"]["controlRevision"],
        1
    );
    assert_eq!(duplicate_local_result, client_a_echo);
    assert_eq!(
        first_broadcast["payload"]["values"]["value_1"],
        json!({ "type": "float", "representation": "f32", "value": 12.0 })
    );
    assert!(no_second_broadcast.is_err());
}

#[tokio::test]
async fn realtime_control_idempotency_key_is_scoped_separately_from_presence() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_presence(&mut client_a, "presence-shared-key", "shared-key").await;
    let presence_ack = next_type(&mut client_a, "command.ack").await;
    let _presence_echo = next_type(&mut client_a, "presence.updated").await;
    let _presence_broadcast = next_type(&mut client_b, "presence.updated").await;

    send_control_command(
        &mut client_a,
        "control-shared-key",
        "shared-key",
        "value_1",
        "in",
        float_message(7.0),
    )
    .await;
    let control_ack = next_type(&mut client_a, "graph.ack").await;
    let control_broadcast = next_type(&mut client_b, "control.emitted").await;

    assert_eq!(presence_ack["payload"]["accepted"], true);
    assert_eq!(control_ack["payload"]["accepted"], true);
    assert_eq!(control_ack["payload"]["cached"], false);
    assert_eq!(control_ack["payload"]["status"], "accepted");
    assert_eq!(
        control_broadcast["payload"]["emitted"][0]["message"],
        float_message(7.0)
    );
}

#[tokio::test]
async fn realtime_control_silent_set_broadcasts_changed_durable_value() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_control_command(
        &mut client_a,
        "control-set-1",
        "control-set-key",
        "value_1",
        "in",
        set_float_message(32.0),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "control.emitted").await;

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["node"]["input"]["changed"], true);
    assert_eq!(broadcast["payload"]["emitted"], json!([]));
    assert_eq!(
        broadcast["payload"]["values"]["value_1"],
        json!({ "type": "float", "representation": "f32", "value": 32.0 })
    );
}

#[tokio::test]
async fn realtime_graph_view_patch_broadcasts_applied_to_attached_clients() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "graph-view-1",
        "graph-view-key",
        view_patch_payload(1, 140.0, 112.0),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "graph.applied").await;

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(ack["payload"]["applied"], true);
    assert_eq!(ack["payload"]["kind"], "view.patch");
    assert_eq!(ack["payload"]["graphRevision"], "1");
    assert_eq!(ack["payload"]["viewRevision"], 2);
    assert!(ack["payload"].get("history").is_none());
    assert!(ack["payload"]["historySummary"]["latestEntryId"].is_string());
    assert_eq!(ack["payload"]["historySummary"]["undoDepth"], 1);
    assert_eq!(ack["payload"]["historySummary"]["redoDepth"], 0);
    assert_eq!(broadcast["payload"]["kind"], "view.patch");
    assert_eq!(broadcast["payload"]["viewRevision"], 2);
    assert_eq!(broadcast["payload"]["graphRevision"], "1");
    assert_eq!(broadcast["payload"]["surfacePath"]["nodeId"], "value_1");
}

#[tokio::test]
async fn realtime_graph_change_set_broadcasts_compact_applied_event() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "graph-add-1",
        "graph-add-key",
        graph_node_add_payload("1", "added_1"),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "graph.applied").await;

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(ack["payload"]["applied"], true);
    assert_eq!(ack["payload"]["kind"], "graph.changeSet");
    assert_eq!(ack["payload"]["baseGraphRevision"], "1");
    assert_eq!(ack["payload"]["graphRevision"], "2");
    assert_eq!(ack["payload"]["viewRevision"], 2);
    assert!(ack["payload"].get("history").is_none());
    assert!(ack["payload"]["historySummary"]["latestEntryId"].is_string());
    assert_eq!(ack["payload"]["historySummary"]["canUndo"], true);
    assert_eq!(ack["payload"]["historySummary"]["canRedo"], false);
    assert_eq!(broadcast["payload"]["kind"], "graph.changeSet");
    assert_eq!(broadcast["payload"]["graphRevision"], "2");
    assert!(broadcast["payload"]["historyEntryId"].is_string());
}

#[tokio::test]
async fn realtime_graph_paste_fragment_and_history_undo_redo_are_ws_commands() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "paste-fragment-1",
        "paste-fragment-key",
        paste_fragment_payload("1", "pasted_ws"),
    )
    .await;
    let paste_ack = next_type(&mut client_a, "graph.ack").await;
    let paste_broadcast = next_type(&mut client_b, "graph.applied").await;

    assert_eq!(paste_ack["payload"]["status"], "accepted");
    assert_eq!(paste_ack["payload"]["kind"], "graph.pasteFragment");
    assert_eq!(paste_ack["payload"]["operation"]["ok"], true);
    assert_eq!(paste_ack["payload"]["operation"]["applied"], true);
    assert_eq!(paste_ack["payload"]["historySummary"]["canUndo"], true);
    assert_eq!(paste_broadcast["payload"]["kind"], "graph.pasteFragment");
    assert_eq!(
        paste_broadcast["payload"]["operation"]["revisionAfter"],
        "2"
    );

    send_graph_command(
        &mut client_a,
        "history-undo-1",
        "history-undo-key",
        history_payload("history.undo"),
    )
    .await;
    let undo_ack = next_type(&mut client_a, "graph.ack").await;
    let undo_broadcast = next_type(&mut client_b, "graph.applied").await;
    assert_eq!(undo_ack["payload"]["status"], "accepted");
    assert_eq!(undo_ack["payload"]["kind"], "history.undo");
    assert_eq!(undo_ack["payload"]["graphRevision"], "3");
    assert_eq!(undo_ack["payload"]["historySummary"]["canRedo"], true);
    assert_eq!(undo_broadcast["payload"]["kind"], "history.undo");

    send_graph_command(
        &mut client_a,
        "history-redo-1",
        "history-redo-key",
        history_payload("history.redo"),
    )
    .await;
    let redo_ack = next_type(&mut client_a, "graph.ack").await;
    let redo_broadcast = next_type(&mut client_b, "graph.applied").await;
    assert_eq!(redo_ack["payload"]["status"], "accepted");
    assert_eq!(redo_ack["payload"]["kind"], "history.redo");
    assert_eq!(redo_ack["payload"]["historySummary"]["canUndo"], true);
    assert_eq!(redo_broadcast["payload"]["kind"], "history.redo");
}

#[tokio::test]
async fn realtime_selection_update_broadcasts_selection_updated() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_json(
        &mut client_a,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "selection.update",
            "messageId": "selection-1",
            "sessionId": "default",
            "commandId": "selection-1",
            "correlationId": "selection-1",
            "idempotencyKey": "selection-key",
            "payload": {
                "target": root_target("1"),
                "selection": {
                    "ranges": [
                        { "kind": "nodes", "nodeIds": ["value_1"] }
                    ],
                    "activeRangeIndex": 0
                },
                "cursor": {
                    "kind": "canvas",
                    "x": 10.0,
                    "y": 20.0
                },
                "ttlMs": 30000
            }
        }),
    )
    .await;

    let ack = next_type(&mut client_a, "command.ack").await;
    let broadcast = next_type(&mut client_b, "selection.updated").await;

    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(broadcast["clientId"], attached_a["clientId"]);
    assert_eq!(broadcast["payload"]["selection"]["sessionId"], "default");
    assert_eq!(
        broadcast["payload"]["selection"]["participantId"],
        attached_a["clientId"]
    );
    assert_eq!(
        broadcast["payload"]["selection"]["selection"]["ranges"][0]["nodeIds"][0],
        "value_1"
    );
}

#[tokio::test]
async fn realtime_graph_duplicate_replays_all_cached_local_events_without_rebroadcast() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "catalog-change-once",
        "catalog-change-key",
        project_patch_interface_node_add_payload("my-patcher", "1"),
    )
    .await;
    let first_ack = next_type(&mut client_a, "graph.ack").await;
    let first_applied = next_type(&mut client_a, "graph.applied").await;
    let first_catalog = next_type(&mut client_a, "nodeCatalog.changed").await;
    let _broadcast_applied = next_type(&mut client_b, "graph.applied").await;
    let _broadcast_catalog = next_type(&mut client_b, "nodeCatalog.changed").await;

    send_graph_command(
        &mut client_a,
        "catalog-change-duplicate",
        "catalog-change-key",
        project_patch_interface_node_add_payload("my-patcher", "1"),
    )
    .await;
    let duplicate_ack = next_type(&mut client_a, "graph.ack").await;
    let duplicate_applied = next_type(&mut client_a, "graph.applied").await;
    let duplicate_catalog = next_type(&mut client_a, "nodeCatalog.changed").await;
    let no_second_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_eq!(duplicate_ack["payload"]["cached"], true);
    assert_eq!(
        duplicate_ack["payload"]["eventCursor"],
        first_ack["payload"]["eventCursor"]
    );
    assert_eq!(duplicate_applied, first_applied);
    assert_eq!(duplicate_catalog, first_catalog);
    assert!(no_second_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_base_revision_conflict_rejects_ack_without_broadcast() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "graph-conflict-1",
        "graph-conflict-key",
        graph_node_add_payload("0", "conflicted_1"),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_eq!(ack["payload"]["status"], "conflict");
    assert_eq!(ack["payload"]["accepted"], false);
    assert_eq!(ack["payload"]["applied"], false);
    assert_eq!(ack["payload"]["conflict"], true);
    assert_eq!(
        ack["payload"]["diagnostics"][0]["code"],
        "collaboration.revision-conflict"
    );
    assert!(ack["payload"].get("history").is_none());
    assert_eq!(
        ack["payload"]["historySummary"]["latestEntryId"],
        Value::Null
    );
    assert_eq!(ack["payload"]["historySummary"]["undoDepth"], 0);
    assert_eq!(ack["payload"]["historySummary"]["redoDepth"], 0);
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn node_catalog_get_exposes_core_and_project_patch_entries() {
    let runtime = spawn_loaded_runtime().await;

    let catalog_json =
        get_json_from_state(&runtime.state, "/v0/sessions/default/node-catalog").await;
    let catalog: NodeCatalogSnapshotV01 =
        serde_json::from_value(catalog_json.clone()).expect("catalog should parse");
    validate_node_catalog_snapshot_v01(&catalog).expect("catalog should validate");

    assert_eq!(catalog_json["schema"], "skenion.node-catalog.snapshot");
    assert_eq!(catalog_json["schemaVersion"], "0.1.0");
    assert!(
        catalog_json["catalogRevision"]["value"]
            .as_str()
            .is_some_and(|value| value.len() == 64)
    );
    assert!(catalog.entries.iter().any(|entry| {
        entry.source == skenion_contracts::NodeCatalogSourceV01::Core
            && entry.definition.id == "object.core.float"
    }));
    let float_entry = catalog
        .entries
        .iter()
        .find(|entry| {
            entry.source == skenion_contracts::NodeCatalogSourceV01::Core
                && entry.definition.id == "object.core.float"
        })
        .expect("first-party float entry should be in the catalog");
    let float_input = float_entry
        .definition
        .ports
        .iter()
        .find(|port| port.id == "in")
        .expect("first-party float entry should include its message input");
    let message_keys = float_input
        .message_keys
        .as_ref()
        .expect("generated first-party float message input should include messageKeys");
    assert!(message_keys.accepted.iter().any(|key| key == "set"));
    assert!(
        message_keys
            .silent
            .as_ref()
            .is_some_and(|keys| keys.iter().any(|key| key == "set"))
    );
    assert!(
        message_keys
            .store
            .as_ref()
            .is_some_and(|keys| keys.iter().any(|key| key == "set"))
    );
    assert!(
        !message_keys
            .trigger
            .as_ref()
            .is_some_and(|keys| keys.iter().any(|key| key == "set"))
    );
    assert!(
        !message_keys
            .emit
            .as_ref()
            .is_some_and(|keys| keys.iter().any(|key| key == "set"))
    );
    let project_patch_entry = catalog
        .entries
        .iter()
        .find(|entry| {
            matches!(
                &entry.source,
                skenion_contracts::NodeCatalogSourceV01::ProjectPatch { patch_id, .. }
                    if patch_id == "my-patcher"
            )
        })
        .expect("project patch entry should be in the catalog");
    assert_eq!(project_patch_entry.display.title, "my-patcher");
    assert_eq!(
        project_patch_entry.display.description.as_deref(),
        Some("my-patcher reusable patch")
    );
    assert!(catalog.entries.iter().all(|entry| {
        matches!(
            entry.source,
            skenion_contracts::NodeCatalogSourceV01::Core
                | skenion_contracts::NodeCatalogSourceV01::ProjectPatch { .. }
        )
    }));
}

#[tokio::test]
async fn node_catalog_revision_changes_when_project_patch_interface_changes() {
    let base_runtime = spawn_loaded_runtime().await;
    let base_catalog =
        get_json_from_state(&base_runtime.state, "/v0/sessions/default/node-catalog").await;

    let changed_state = RuntimeServerState::default();
    let changed_app = runtime_router_with_state(changed_state.clone());
    let mut changed_project = sample_project_document_current();
    changed_project["patchLibrary"][0] =
        project_patch_definition_with_float_interface_current_json("my-patcher");
    let response = changed_app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v0/sessions/default/load")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    session_load_request(changed_project).to_string(),
                ))
                .expect("load request builds"),
        )
        .await
        .expect("load request succeeds");
    assert!(response.status().is_success());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("load response body should be readable");
    let load_response: Value =
        serde_json::from_slice(&body).expect("load response body should be JSON");
    assert_eq!(load_response["ok"], true, "load failed: {load_response}");

    let changed_catalog =
        get_json_from_state(&changed_state, "/v0/sessions/default/node-catalog").await;

    assert_ne!(
        base_catalog["catalogRevision"]["value"],
        changed_catalog["catalogRevision"]["value"]
    );
    assert_ne!(
        base_catalog["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["source"]["patchId"] == "my-patcher")
            .unwrap()["source"]["interfaceDigest"]["value"],
        changed_catalog["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["source"]["patchId"] == "my-patcher")
            .unwrap()["source"]["interfaceDigest"]["value"]
    );
}

#[tokio::test]
async fn repeated_node_catalog_get_is_snapshot_only() {
    let runtime = spawn_loaded_runtime().await;
    let before = {
        let record = runtime.state.sessions.get_or_create("default");
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        (
            session.snapshot(),
            session.history().entries.len(),
            runtime.state.logs.snapshot().events.len(),
        )
    };

    let first = get_json_from_state(&runtime.state, "/v0/sessions/default/node-catalog").await;
    let second = get_json_from_state(&runtime.state, "/v0/sessions/default/node-catalog").await;

    let after = {
        let record = runtime.state.sessions.get_or_create("default");
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        (
            session.snapshot(),
            session.history().entries.len(),
            runtime.state.logs.snapshot().events.len(),
        )
    };

    assert_eq!(first, second);
    assert_eq!(before.0.session_revision, after.0.session_revision);
    assert_eq!(before.0.graph_revision(), after.0.graph_revision());
    assert_eq!(before.0.project, after.0.project);
    assert_eq!(before.1, after.1);
    assert_eq!(before.2, after.2);
}

#[tokio::test]
async fn realtime_attach_hydrates_node_catalog_by_requested_mode() {
    let runtime = spawn_loaded_runtime().await;
    let mut default_socket = connect_session(&runtime, "default").await;
    let default_attached = attach(&mut default_socket, "hello-default", None).await;

    assert_eq!(default_attached["type"], "session.attached");
    assert_eq!(
        default_attached["payload"]["nodeCatalog"]["status"],
        "notRequested"
    );
    assert!(default_attached["payload"]["nodeCatalog"]["catalogRevision"].is_object());
    assert!(
        default_attached["payload"]["nodeCatalog"]
            .get("snapshot")
            .is_none()
    );

    let mut always_socket = connect_session(&runtime, "default").await;
    let always_attached =
        attach_with_node_catalog(&mut always_socket, "hello-always", "always", None).await;
    let known_revision = always_attached["payload"]["nodeCatalog"]["catalogRevision"].clone();

    assert_eq!(
        always_attached["payload"]["nodeCatalog"]["status"],
        "included"
    );
    assert_eq!(
        always_attached["payload"]["nodeCatalog"]["snapshot"]["catalogRevision"],
        known_revision
    );
    assert!(always_attached["payload"]["nodeCatalog"]["snapshot"]["entries"].is_array());

    let mut unchanged_socket = connect_session(&runtime, "default").await;
    let unchanged_attached = attach_with_node_catalog(
        &mut unchanged_socket,
        "hello-if-changed",
        "ifChanged",
        Some(known_revision),
    )
    .await;

    assert_eq!(
        unchanged_attached["payload"]["nodeCatalog"]["status"],
        "unchanged"
    );
    assert!(
        unchanged_attached["payload"]["nodeCatalog"]
            .get("snapshot")
            .is_none()
    );
}

#[tokio::test]
async fn realtime_sync_required_hydrates_node_catalog_by_requested_mode() {
    let runtime = spawn_loaded_runtime().await;
    let catalog = get_json_from_state(&runtime.state, "/v0/sessions/default/node-catalog").await;
    let known_revision = catalog["catalogRevision"].clone();
    let mut initial_socket = connect_session(&runtime, "default").await;
    let initial_attached = attach(&mut initial_socket, "hello-initial", None).await;
    let initial_cursor = initial_attached["payload"]["globalCursor"]
        .as_str()
        .expect("attached includes global cursor");
    let (incarnation, _) = initial_cursor
        .rsplit_once(':')
        .expect("cursor has sequence");
    let unknown_cursor = format!("{incarnation}:999");
    initial_socket
        .close(None)
        .await
        .unwrap_or_else(|error: TungsteniteError| panic!("initial socket closes: {error}"));

    let mut always_socket = connect_session(&runtime, "default").await;
    let always_sync = attach_with_payload(
        &mut always_socket,
        "hello-sync-always",
        json!({
            "clientId": "client-hint",
            "windowId": "window-hint",
            "hints": { "label": "test" },
            "lastCursor": unknown_cursor,
            "nodeCatalog": { "mode": "always" }
        }),
    )
    .await;

    assert_eq!(always_sync["type"], "session.syncRequired");
    assert_eq!(
        always_sync["payload"]["diagnostic"]["code"],
        "realtime.cursor.unknown"
    );
    assert_eq!(always_sync["payload"]["nodeCatalog"]["status"], "included");
    assert_eq!(
        always_sync["payload"]["nodeCatalog"]["catalogRevision"],
        known_revision
    );
    assert_eq!(
        always_sync["payload"]["nodeCatalog"]["snapshot"]["catalogRevision"],
        known_revision
    );
    assert!(always_sync["payload"]["nodeCatalog"]["snapshot"]["entries"].is_array());

    let mut unchanged_socket = connect_session(&runtime, "default").await;
    let unchanged_sync = attach_with_payload(
        &mut unchanged_socket,
        "hello-sync-if-changed",
        json!({
            "clientId": "client-hint",
            "windowId": "window-hint",
            "hints": { "label": "test" },
            "lastCursor": unknown_cursor,
            "nodeCatalog": {
                "mode": "ifChanged",
                "knownRevision": known_revision
            }
        }),
    )
    .await;

    assert_eq!(unchanged_sync["type"], "session.syncRequired");
    assert_eq!(
        unchanged_sync["payload"]["diagnostic"]["code"],
        "realtime.cursor.unknown"
    );
    assert_eq!(
        unchanged_sync["payload"]["nodeCatalog"]["status"],
        "unchanged"
    );
    assert_eq!(
        unchanged_sync["payload"]["nodeCatalog"]["catalogRevision"],
        catalog["catalogRevision"]
    );
    assert!(
        unchanged_sync["payload"]["nodeCatalog"]
            .get("snapshot")
            .is_none()
    );
}

#[tokio::test]
async fn realtime_node_catalog_request_returns_snapshot_or_unchanged() {
    let runtime = spawn_loaded_runtime().await;
    let mut socket = connect_session(&runtime, "default").await;
    let _attached = attach(&mut socket, "hello-1", None).await;

    send_node_catalog_request(&mut socket, "catalog-request-1", None).await;
    let snapshot = next_type(&mut socket, "nodeCatalog.snapshot").await;
    let known_revision = snapshot["payload"]["catalogRevision"]["value"]
        .as_str()
        .expect("snapshot response includes revision value")
        .to_owned();

    assert_eq!(snapshot["payload"]["status"], "included");
    assert_eq!(
        snapshot["payload"]["snapshot"]["catalogRevision"],
        snapshot["payload"]["catalogRevision"]
    );

    send_node_catalog_request(
        &mut socket,
        "catalog-request-2",
        Some(Value::String(known_revision)),
    )
    .await;
    let unchanged = next_type(&mut socket, "nodeCatalog.unchanged").await;

    assert_eq!(unchanged["payload"]["status"], "unchanged");
    assert!(unchanged["payload"].get("snapshot").is_none());
    assert_eq!(
        unchanged["payload"]["catalogRevision"],
        snapshot["payload"]["catalogRevision"]
    );
}

#[tokio::test]
async fn realtime_node_catalog_revision_ignores_presence_and_selection_updates() {
    for (name, presence) in [
        ("presence", json!({ "state": "active" })),
        (
            "selection",
            json!({
                "state": "active",
                "selection": { "nodeIds": ["value_1"] }
            }),
        ),
    ] {
        let runtime = spawn_loaded_runtime().await;
        let mut client_a = connect_session(&runtime, "default").await;
        let mut client_b = connect_session(&runtime, "default").await;
        let _attached_a = attach(&mut client_a, &format!("hello-a-{name}"), None).await;
        let _attached_b = attach(&mut client_b, &format!("hello-b-{name}"), None).await;
        let before = get_json_from_state(&runtime.state, "/v0/sessions/default/node-catalog").await;

        send_presence_payload(
            &mut client_a,
            &format!("{name}-update"),
            &format!("{name}-key"),
            presence,
        )
        .await;
        let ack = next_type(&mut client_a, "command.ack").await;
        let _client_a_echo = next_type(&mut client_a, "presence.updated").await;
        let _client_b_broadcast = next_type(&mut client_b, "presence.updated").await;
        let no_catalog_change = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
        let after = get_json_from_state(&runtime.state, "/v0/sessions/default/node-catalog").await;

        assert_eq!(ack["payload"]["accepted"], true, "{name}");
        assert_eq!(
            before["catalogRevision"], after["catalogRevision"],
            "{name} must not change catalogRevision"
        );
        assert!(
            no_catalog_change.is_err(),
            "{name} must not emit nodeCatalog.changed"
        );
    }
}

#[tokio::test]
async fn realtime_node_catalog_revision_ignores_non_catalog_graph_view_and_input_changes() {
    for (name, payload, expected_event) in [
        ("view", view_patch_payload(1, 140.0, 112.0), "graph.applied"),
        (
            "root-node",
            graph_node_add_payload("1", "added_1"),
            "graph.applied",
        ),
        (
            "edge",
            graph_edge_disconnect_payload("1", "edge_value_target"),
            "graph.applied",
        ),
        (
            "params",
            node_update_payload("1", "value_1", json!({ "label": "Stable" })),
            "graph.applied",
        ),
        (
            "input",
            node_input_payload("value_1", "in", float_message(12.0)),
            "control.emitted",
        ),
        (
            "project-patch-internal",
            project_patch_internal_node_add_payload("my-patcher", "1"),
            "graph.applied",
        ),
    ] {
        let runtime = spawn_loaded_runtime().await;
        let mut client_a = connect_session(&runtime, "default").await;
        let mut client_b = connect_session(&runtime, "default").await;
        let _attached_a = attach(&mut client_a, &format!("hello-a-{name}"), None).await;
        let _attached_b = attach(&mut client_b, &format!("hello-b-{name}"), None).await;
        let before = get_json_from_state(&runtime.state, "/v0/sessions/default/node-catalog").await;

        send_graph_command(
            &mut client_a,
            &format!("{name}-command"),
            &format!("{name}-key"),
            payload,
        )
        .await;
        let ack = next_type(&mut client_a, "graph.ack").await;
        let _broadcast = next_type(&mut client_b, expected_event).await;
        let no_catalog_change = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
        let after = get_json_from_state(&runtime.state, "/v0/sessions/default/node-catalog").await;

        assert_eq!(ack["payload"]["status"], "accepted", "{name}");
        assert_eq!(
            before["catalogRevision"], after["catalogRevision"],
            "{name} must not change catalogRevision"
        );
        assert!(
            no_catalog_change.is_err(),
            "{name} must not emit nodeCatalog.changed"
        );
    }
}

#[tokio::test]
async fn realtime_node_catalog_changed_fires_for_project_patch_interface_change() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;
    let before = get_json_from_state(&runtime.state, "/v0/sessions/default/node-catalog").await;

    send_graph_command(
        &mut client_a,
        "patch-interface-1",
        "patch-interface-key",
        project_patch_interface_node_add_payload("my-patcher", "1"),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let _graph = next_type(&mut client_b, "graph.applied").await;
    let changed = next_type(&mut client_b, "nodeCatalog.changed").await;
    let after = get_json_from_state(&runtime.state, "/v0/sessions/default/node-catalog").await;

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_ne!(before["catalogRevision"], after["catalogRevision"]);
    assert_eq!(
        changed["payload"]["catalogRevision"],
        after["catalogRevision"]
    );
    assert_eq!(
        changed["payload"]["snapshot"]["catalogRevision"],
        after["catalogRevision"]
    );
}

#[test]
fn runtime_source_does_not_import_contract_node_graph_command_wire_types() {
    let needle = ["Node", "Graph", "Command"].join("");
    assert_source_tree_lacks(Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &needle);
}

#[tokio::test]
async fn realtime_graph_node_create_osc_materializes_node_through_ws_command() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-create-osc-1",
        "node-create-osc-key",
        node_create_payload_with_params(
            "1",
            "osc_1",
            "osc~ 220",
            Some(json!({ "frequency": 880.0, "label": "Lead" })),
        ),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "graph.applied").await;
    let project = loaded_project_json(&runtime);
    let node = graph_node(&project, "osc_1");

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(ack["payload"]["applied"], true);
    assert_eq!(ack["payload"]["kind"], "node.create");
    assert_eq!(ack["payload"]["graphRevision"], "2");
    assert_eq!(ack["payload"]["node"]["nodeId"], "osc_1");
    assert_eq!(
        ack["payload"]["node"]["resolution"]["resolvedKind"],
        "object.core.audio.osc"
    );
    assert_eq!(
        ack["payload"]["node"]["resolution"]["params"]["frequency"],
        220.0
    );
    assert_eq!(broadcast["payload"]["kind"], "node.create");
    assert_eq!(broadcast["payload"]["node"]["nodeId"], "osc_1");
    assert_eq!(node["kind"], "object.core.audio.osc");
    assert_eq!(node["objectText"], "osc~ 220");
    assert_eq!(node["params"]["frequency"], 880.0);
    assert_eq!(node["params"]["label"], "Lead");
    assert_eq!(
        project["viewState"]["canvas"]["nodes"]["osc_1"],
        json!({ "x": 420.0, "y": 144.0 })
    );
}

#[tokio::test]
async fn realtime_graph_node_create_generates_slugged_ids_when_request_id_is_absent() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-create-auto-1",
        "node-create-auto-key-1",
        node_create_payload_without_requested_id("1", "osc~ 220"),
    )
    .await;
    let first_ack = next_type(&mut client_a, "graph.ack").await;
    let first_broadcast = next_type(&mut client_b, "graph.applied").await;

    let mut second_payload = node_create_payload_without_requested_id("2", "osc~ 220");
    second_payload
        .as_object_mut()
        .expect("node.create payload should be an object")
        .remove("baseSessionRevision");
    send_graph_command(
        &mut client_a,
        "node-create-auto-2",
        "node-create-auto-key-2",
        second_payload,
    )
    .await;
    let second_ack = next_type(&mut client_a, "graph.ack").await;
    let second_broadcast = next_type(&mut client_b, "graph.applied").await;
    let project = loaded_project_json(&runtime);

    assert_eq!(first_ack["payload"]["status"], "accepted");
    assert_eq!(first_ack["payload"]["node"]["nodeId"], "osc_220");
    assert_eq!(first_broadcast["payload"]["node"]["nodeId"], "osc_220");
    assert_eq!(second_ack["payload"]["status"], "accepted");
    assert_eq!(second_ack["payload"]["node"]["nodeId"], "osc_220_2");
    assert_eq!(second_broadcast["payload"]["node"]["nodeId"], "osc_220_2");
    assert!(graph_node(&project, "osc_220").is_object());
    assert!(graph_node(&project, "osc_220_2").is_object());
}

#[tokio::test]
async fn realtime_graph_node_resolve_uses_runtime_registry_candidates() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    for (index, (object_text, expected_kind, expected_param)) in [
        ("* .3", "object.core.operator.mul", Some(json!(0.3))),
        ("* 3", "object.core.operator.mul", Some(json!(3.0))),
        ("+", "object.core.operator.add", Some(json!(0.0))),
        ("*", "object.core.operator.mul", Some(json!(0.0))),
        ("osc~", "object.core.audio.osc", Some(json!(440.0))),
        ("*~", "object.core.audio.operator.mul", None),
        (
            "p my-patcher",
            "object.project.patch.my-patcher",
            Some(json!("my-patcher")),
        ),
        (
            "my-patcher",
            "object.project.patch.my-patcher",
            Some(json!("my-patcher")),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        send_graph_command(
            &mut client_a,
            &format!("node-resolve-{index}"),
            &format!("node-resolve-key-{index}"),
            node_resolve_payload("1", object_text),
        )
        .await;
        let ack = next_type(&mut client_a, "graph.ack").await;

        assert_eq!(ack["payload"]["status"], "accepted", "{object_text}");
        assert_eq!(ack["payload"]["applied"], false, "{object_text}");
        assert_eq!(
            ack["payload"]["node"]["resolution"]["resolvedKind"], expected_kind,
            "{object_text}"
        );
        assert_eq!(
            ack["payload"]["node"]["resolution"]["candidateCount"], 1,
            "{object_text}"
        );
        if let Some(expected_param) = expected_param {
            let param_key = if expected_kind.starts_with("object.project.patch.") {
                "patchRef"
            } else if expected_kind == "object.core.audio.osc" {
                "frequency"
            } else {
                "right"
            };
            assert_eq!(
                ack["payload"]["node"]["resolution"]["params"][param_key], expected_param,
                "{object_text}"
            );
        }
    }

    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_node_resolve_unknown_returns_diagnostics_without_apply() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-resolve-unknown-1",
        "node-resolve-unknown-key",
        node_resolve_payload("1", "mystery.object 1"),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
    let project = loaded_project_json(&runtime);

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(ack["payload"]["applied"], false);
    assert_eq!(ack["payload"]["graphRevision"], "1");
    assert_eq!(ack["payload"]["node"]["resolution"]["resolved"], false);
    assert_eq!(
        ack["payload"]["node"]["resolution"]["diagnostics"][0]["code"],
        "object-text.unresolved"
    );
    assert_eq!(
        ack["payload"]["diagnostics"][0]["code"],
        "object-text.unresolved"
    );
    assert_eq!(
        project["graph"]["nodes"]
            .as_array()
            .expect("nodes should be array")
            .len(),
        2
    );
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_node_commands_validate_targets_before_session_mutation() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    let mut missing_target = node_create_payload("1", "missing_target_1", "osc~ 220");
    missing_target
        .as_object_mut()
        .expect("node.create payload should be an object")
        .remove("target");
    send_graph_command(
        &mut client_a,
        "node-create-missing-target",
        "node-create-missing-target-key",
        missing_target,
    )
    .await;
    let missing_target_ack = next_type(&mut client_a, "graph.ack").await;

    let mut mismatched_target_revision =
        node_create_payload("1", "mismatched_target_revision_1", "osc~ 220");
    mismatched_target_revision
        .as_object_mut()
        .expect("node.create payload should be an object")
        .insert("baseGraphRevision".to_owned(), json!("2"));
    send_graph_command(
        &mut client_a,
        "node-create-target-revision-mismatch",
        "node-create-target-revision-mismatch-key",
        mismatched_target_revision,
    )
    .await;
    let target_revision_ack = next_type(&mut client_a, "graph.ack").await;

    send_graph_command(
        &mut client_a,
        "node-resolve-missing-graph",
        "node-resolve-missing-graph-key",
        node_resolve_payload_with_target(
            "1",
            "osc~ 220",
            project_patch_target("missing-patcher", "1"),
        ),
    )
    .await;
    let missing_graph_ack = next_type(&mut client_a, "graph.ack").await;
    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
    let project = loaded_project_json(&runtime);

    assert_eq!(missing_target_ack["payload"]["status"], "rejected");
    assert_eq!(
        missing_target_ack["payload"]["diagnostics"][0]["code"],
        "graph.command.target-required"
    );
    assert_eq!(target_revision_ack["payload"]["status"], "conflict");
    assert_eq!(target_revision_ack["payload"]["conflict"], true);
    assert_eq!(
        target_revision_ack["payload"]["diagnostics"][0]["code"],
        "graph.command.target-revision-conflict"
    );
    assert_eq!(missing_graph_ack["payload"]["status"], "rejected");
    assert_eq!(
        missing_graph_ack["payload"]["diagnostics"][0]["code"],
        "node.target.missing-graph"
    );
    assert_eq!(project["graph"]["revision"], "1");
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_node_create_missing_materializes_diagnostic_node_by_default() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-create-missing-1",
        "node-create-missing-key",
        node_create_payload("1", "missing_1", "mystery.object 1"),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "graph.applied").await;
    let project = loaded_project_json(&runtime);
    let node = graph_node(&project, "missing_1");

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["applied"], true);
    assert_eq!(
        ack["payload"]["node"]["unresolvedPolicy"],
        "materialize-diagnostic"
    );
    assert_eq!(
        ack["payload"]["node"]["resolution"]["diagnostics"][0]["code"],
        "object-text.unresolved"
    );
    assert_eq!(broadcast["payload"]["node"]["nodeId"], "missing_1");
    assert_eq!(node["kind"], "object.core.unresolved");
    assert_eq!(node["objectText"], "mystery.object 1");
    assert_eq!(node["params"]["diagnosticCode"], "object-text.unresolved");
    assert_eq!(node["params"]["candidateCount"], 0);
}

#[tokio::test]
async fn realtime_graph_node_create_ambiguous_shortcut_materializes_diagnostic_node() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-create-ambiguous-1",
        "node-create-ambiguous-key",
        node_create_payload("1", "ambiguous_1", "add"),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "graph.applied").await;
    let project = loaded_project_json(&runtime);
    let node = graph_node(&project, "ambiguous_1");

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["applied"], true);
    assert_eq!(
        ack["payload"]["node"]["resolution"]["diagnostics"][0]["code"],
        "object-text.ambiguous"
    );
    assert_eq!(ack["payload"]["node"]["resolution"]["candidateCount"], 2);
    assert_eq!(broadcast["payload"]["node"]["nodeId"], "ambiguous_1");
    assert_eq!(node["kind"], "object.core.unresolved");
    assert_eq!(node["params"]["diagnosticCode"], "object-text.ambiguous");
    assert_eq!(node["params"]["candidateCount"], 2);
}

#[tokio::test]
async fn realtime_graph_node_create_rejects_no_project_duplicate_and_patch_view_edges() {
    let unloaded = spawn_runtime().await;
    let mut unloaded_socket = connect_session(&unloaded, "default").await;
    let _unloaded_attached = attach(&mut unloaded_socket, "hello-unloaded", None).await;

    let mut no_project_payload = node_create_payload("1", "no_project_1", "osc~ 220");
    no_project_payload
        .as_object_mut()
        .expect("node.create payload should be an object")
        .remove("baseSessionRevision");
    send_graph_command(
        &mut unloaded_socket,
        "node-create-no-project",
        "node-create-no-project-key",
        no_project_payload,
    )
    .await;
    let no_project_ack = next_type(&mut unloaded_socket, "graph.ack").await;

    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-create-duplicate",
        "node-create-duplicate-key",
        node_create_payload("1", "value_1", "osc~ 220"),
    )
    .await;
    let duplicate_ack = next_type(&mut client_a, "graph.ack").await;

    send_graph_command(
        &mut client_a,
        "node-create-patch-view",
        "node-create-patch-view-key",
        node_create_project_patch_payload("my-patcher", "1", "patch_osc_1", "osc~ 220"),
    )
    .await;
    let patch_view_ack = next_type(&mut client_a, "graph.ack").await;
    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
    let project = loaded_project_json(&runtime);

    assert_eq!(no_project_ack["payload"]["status"], "rejected");
    assert_eq!(
        no_project_ack["payload"]["diagnostics"][0]["code"],
        "node.target.no-project"
    );
    assert_eq!(duplicate_ack["payload"]["status"], "rejected");
    assert_eq!(
        duplicate_ack["payload"]["diagnostics"][0]["code"],
        "node.create.node-id-conflict"
    );
    assert_eq!(patch_view_ack["payload"]["status"], "rejected");
    assert_eq!(
        patch_view_ack["payload"]["diagnostics"][0]["code"],
        "collaboration.patch-view-unsupported"
    );
    assert_eq!(project["graph"]["revision"], "1");
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_node_replace_preserves_node_id_and_prunes_invalid_incident_edges() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-replace-osc-1",
        "node-replace-osc-key",
        node_replace_payload_with_params(
            "1",
            "value_1",
            "osc~ 330",
            Some(json!({ "frequency": 660.0, "label": "Retuned" })),
        ),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "graph.applied").await;
    let project = loaded_project_json(&runtime);
    let node = graph_node(&project, "value_1");
    let edge_ids = graph_edge_ids(&project);

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(ack["payload"]["applied"], true);
    assert_eq!(ack["payload"]["node"]["nodeId"], "value_1");
    assert_eq!(
        ack["payload"]["node"]["droppedEdgeIds"],
        json!(["edge_value_target"])
    );
    assert_eq!(
        ack["payload"]["diagnostics"][0]["code"],
        "node.replace.incident-edges-dropped"
    );
    assert_eq!(
        ack["payload"]["node"]["resolution"]["params"]["frequency"],
        330.0
    );
    assert_eq!(broadcast["payload"]["node"]["nodeId"], "value_1");
    assert_eq!(node["id"], "value_1");
    assert_eq!(node["kind"], "object.core.audio.osc");
    assert_eq!(node["objectText"], "osc~ 330");
    assert_eq!(node["params"]["frequency"], 660.0);
    assert_eq!(node["params"]["label"], "Retuned");
    assert!(
        !edge_ids
            .iter()
            .any(|edge_id| edge_id == "edge_value_target")
    );
    assert!(graph_node(&project, "target_1").is_object());
}

#[tokio::test]
async fn realtime_graph_node_replace_rejects_invalid_incident_edge_policies_without_mutation() {
    for (policy, expected_code) in [
        ("reject", "node.replace.invalid-incident-edge"),
        (
            "preserve-diagnostic",
            "node.replace.preserve-diagnostic-unsupported",
        ),
    ] {
        let runtime = spawn_loaded_runtime().await;
        let mut client_a = connect_session(&runtime, "default").await;
        let mut client_b = connect_session(&runtime, "default").await;
        let _attached_a = attach(&mut client_a, &format!("hello-a-{policy}"), None).await;
        let _attached_b = attach(&mut client_b, &format!("hello-b-{policy}"), None).await;

        send_graph_command(
            &mut client_a,
            &format!("node-replace-policy-{policy}"),
            &format!("node-replace-policy-key-{policy}"),
            node_replace_payload_with_policy("1", "value_1", "osc~ 330", policy),
        )
        .await;
        let ack = next_type(&mut client_a, "graph.ack").await;
        let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
        let project = loaded_project_json(&runtime);
        let node = graph_node(&project, "value_1");

        assert_eq!(ack["payload"]["status"], "rejected", "{policy}");
        assert_eq!(ack["payload"]["accepted"], false, "{policy}");
        assert_eq!(
            ack["payload"]["diagnostics"][0]["code"], expected_code,
            "{policy}"
        );
        assert_eq!(project["graph"]["revision"], "1", "{policy}");
        assert_eq!(node["kind"], "object.core.float", "{policy}");
        assert!(
            graph_edge_ids(&project)
                .iter()
                .any(|edge_id| edge_id == "edge_value_target"),
            "{policy}"
        );
        assert!(no_broadcast.is_err(), "{policy}");
    }
}

#[tokio::test]
async fn realtime_graph_node_delete_removes_node_and_incident_edges() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-delete-1",
        "node-delete-key",
        node_delete_payload("1", "value_1"),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "graph.applied").await;
    let project = loaded_project_json(&runtime);
    let edge_ids = graph_edge_ids(&project);

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["applied"], true);
    assert_eq!(ack["payload"]["kind"], "node.delete");
    assert_eq!(ack["payload"]["graphRevision"], "2");
    assert_eq!(ack["payload"]["viewRevision"], 2);
    assert_eq!(
        ack["payload"]["sessionRevision"],
        broadcast["payload"]["sessionRevision"]
    );
    assert!(
        ack["payload"]["sessionRevision"]
            .as_u64()
            .is_some_and(|revision| revision > 1)
    );
    assert!(ack["payload"]["historySummary"]["latestEntryId"].is_string());
    assert_eq!(ack["payload"]["historySummary"]["canUndo"], true);
    assert_eq!(ack["payload"]["historySummary"]["canRedo"], false);
    assert_eq!(ack["payload"]["historySummary"]["undoDepth"], 1);
    assert_eq!(ack["payload"]["historySummary"]["redoDepth"], 0);
    assert_eq!(ack["payload"]["node"]["nodeId"], "value_1");
    assert_eq!(
        ack["payload"]["node"]["droppedEdgeIds"],
        json!(["edge_value_target"])
    );
    assert_eq!(broadcast["payload"]["kind"], "node.delete");
    assert_eq!(broadcast["payload"]["graphRevision"], "2");
    assert_eq!(broadcast["payload"]["viewRevision"], 2);
    assert_eq!(
        broadcast["payload"]["historyEntryId"],
        ack["payload"]["historySummary"]["latestEntryId"]
    );
    assert_eq!(broadcast["payload"]["node"]["nodeId"], "value_1");
    assert_eq!(project["graph"]["revision"], "2");
    assert!(graph_node_option(&project, "value_1").is_none());
    assert!(
        !edge_ids
            .iter()
            .any(|edge_id| edge_id == "edge_value_target")
    );
    assert!(
        !project["viewState"]["canvas"]["nodes"]
            .as_object()
            .expect("canvas nodes should be object")
            .contains_key("value_1")
    );
}

#[tokio::test]
async fn realtime_graph_node_update_merges_persisted_params_only() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-update-1",
        "node-update-key",
        node_update_payload(
            "1",
            "value_1",
            json!({ "label": "Lead", "storedValue": 32.0 }),
        ),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let broadcast = next_type(&mut client_b, "graph.applied").await;
    let project = loaded_project_json(&runtime);
    let node = graph_node(&project, "value_1");

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["applied"], true);
    assert_eq!(ack["payload"]["kind"], "node.update");
    assert_eq!(ack["payload"]["node"]["nodeId"], "value_1");
    assert_eq!(ack["payload"]["graphRevision"], "2");
    assert_eq!(ack["payload"]["viewRevision"], 1);
    assert_eq!(
        ack["payload"]["sessionRevision"],
        broadcast["payload"]["sessionRevision"]
    );
    assert!(
        ack["payload"]["sessionRevision"]
            .as_u64()
            .is_some_and(|revision| revision > 1)
    );
    assert!(ack["payload"]["historySummary"]["latestEntryId"].is_string());
    assert_eq!(ack["payload"]["historySummary"]["canUndo"], true);
    assert_eq!(ack["payload"]["historySummary"]["canRedo"], false);
    assert_eq!(ack["payload"]["historySummary"]["undoDepth"], 1);
    assert_eq!(ack["payload"]["historySummary"]["redoDepth"], 0);
    assert_eq!(broadcast["payload"]["kind"], "node.update");
    assert_eq!(broadcast["payload"]["graphRevision"], "2");
    assert_eq!(broadcast["payload"]["viewRevision"], 1);
    assert_eq!(
        broadcast["payload"]["historyEntryId"],
        ack["payload"]["historySummary"]["latestEntryId"]
    );
    assert_eq!(broadcast["payload"]["node"]["nodeId"], "value_1");
    assert_eq!(project["graph"]["revision"], "2");
    assert_eq!(node["params"]["label"], "Lead");
    assert_eq!(node["params"]["storedValue"], 32.0);
}

#[tokio::test]
async fn realtime_graph_node_delete_missing_node_rejects_without_broadcast() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-delete-missing-1",
        "node-delete-missing-key",
        node_delete_payload("1", "missing_1"),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
    let project = loaded_project_json(&runtime);

    assert_eq!(ack["payload"]["status"], "rejected");
    assert_eq!(ack["payload"]["accepted"], false);
    assert_eq!(ack["payload"]["applied"], false);
    assert_eq!(ack["payload"]["kind"], "node.delete");
    assert_eq!(ack["payload"]["node"]["nodeId"], "missing_1");
    assert_eq!(ack["payload"]["graphRevision"], "1");
    assert_eq!(ack["payload"]["viewRevision"], 1);
    assert_eq!(
        ack["payload"]["historySummary"]["latestEntryId"],
        Value::Null
    );
    assert_eq!(ack["payload"]["historySummary"]["undoDepth"], 0);
    assert_eq!(ack["payload"]["historySummary"]["redoDepth"], 0);
    assert_eq!(
        ack["payload"]["diagnostics"][0]["code"],
        "node.delete.node-missing"
    );
    assert_eq!(project["graph"]["revision"], "1");
    assert_eq!(
        project["graph"]["nodes"]
            .as_array()
            .expect("nodes should be array")
            .len(),
        2
    );
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_node_update_missing_node_rejects_without_broadcast() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-update-missing-1",
        "node-update-missing-key",
        node_update_payload("1", "missing_1", json!({ "label": "Missing" })),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
    let project = loaded_project_json(&runtime);

    assert_eq!(ack["payload"]["status"], "rejected");
    assert_eq!(ack["payload"]["accepted"], false);
    assert_eq!(ack["payload"]["applied"], false);
    assert_eq!(ack["payload"]["kind"], "node.update");
    assert_eq!(ack["payload"]["node"]["nodeId"], "missing_1");
    assert_eq!(ack["payload"]["graphRevision"], "1");
    assert_eq!(ack["payload"]["viewRevision"], 1);
    assert_eq!(
        ack["payload"]["historySummary"]["latestEntryId"],
        Value::Null
    );
    assert_eq!(ack["payload"]["historySummary"]["undoDepth"], 0);
    assert_eq!(ack["payload"]["historySummary"]["redoDepth"], 0);
    assert_eq!(
        ack["payload"]["diagnostics"][0]["code"],
        "node.update.node-missing"
    );
    assert_eq!(project["graph"]["revision"], "1");
    assert_eq!(
        project["graph"]["nodes"]
            .as_array()
            .expect("nodes should be array")
            .len(),
        2
    );
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_node_update_rejects_empty_params_without_broadcast() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-update-empty-1",
        "node-update-empty-key",
        node_update_payload("1", "value_1", json!({})),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_eq!(ack["payload"]["status"], "rejected");
    assert_eq!(ack["payload"]["accepted"], false);
    assert_eq!(
        ack["payload"]["diagnostics"][0]["code"],
        "graph.command.params-required"
    );
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_node_input_invokes_control_path_without_graph_applied() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-input-1",
        "node-input-key",
        node_input_payload("value_1", "in", float_message(12.0)),
    )
    .await;
    let first_ack = next_type(&mut client_a, "graph.ack").await;
    let client_a_echo = next_json(&mut client_a).await;
    let first_broadcast = next_json(&mut client_b).await;
    let no_first_extra = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
    let control_values = loaded_control_values_json(&runtime);

    assert_eq!(first_ack["payload"]["status"], "accepted");
    assert_eq!(first_ack["payload"]["accepted"], true);
    assert_eq!(first_ack["payload"]["applied"], false);
    assert_eq!(first_ack["payload"]["kind"], "node.input");
    assert_eq!(first_ack["payload"]["graphRevision"], "1");
    assert_eq!(first_ack["payload"]["historySummary"]["undoDepth"], 0);
    assert_eq!(first_ack["payload"]["node"]["nodeId"], "value_1");
    assert_eq!(first_ack["payload"]["node"]["input"]["accepted"], true);
    assert_eq!(first_ack["payload"]["node"]["input"]["changed"], true);
    assert_eq!(first_ack["payload"]["node"]["input"]["portId"], "in");
    assert_eq!(client_a_echo["type"], "control.emitted");
    assert_eq!(first_broadcast["type"], "control.emitted");
    assert_eq!(first_broadcast["payload"]["request"]["nodeId"], "value_1");
    assert_eq!(first_broadcast["payload"]["request"]["portId"], "in");
    assert_eq!(
        first_broadcast["payload"]["emitted"][0]["message"],
        float_message(12.0)
    );
    assert_eq!(
        first_broadcast["payload"]["values"]["value_1"],
        json!({ "type": "float", "representation": "f32", "value": 12.0 })
    );
    assert_eq!(
        control_values["value_1"],
        json!({ "type": "float", "representation": "f32", "value": 12.0 })
    );
    assert_eq!(
        control_values["target_1"],
        json!({ "type": "float", "representation": "f32", "value": 12.0 })
    );
    assert!(no_first_extra.is_err());

    send_graph_command(
        &mut client_a,
        "node-input-duplicate",
        "node-input-key",
        node_input_payload("value_1", "in", float_message(24.0)),
    )
    .await;
    let duplicate_ack = next_type(&mut client_a, "graph.ack").await;
    let duplicate_local_result = next_json(&mut client_a).await;
    let no_second_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
    let control_values_after_duplicate = loaded_control_values_json(&runtime);

    assert_ne!(duplicate_ack["messageId"], first_ack["messageId"]);
    assert_eq!(duplicate_ack["payload"]["accepted"], true);
    assert_eq!(duplicate_ack["payload"]["cached"], true);
    assert_eq!(
        duplicate_ack["payload"]["eventCursor"],
        first_ack["payload"]["eventCursor"]
    );
    assert_eq!(
        duplicate_ack["payload"]["node"]["input"]["controlRevision"],
        first_ack["payload"]["node"]["input"]["controlRevision"]
    );
    assert_eq!(duplicate_local_result, client_a_echo);
    assert_eq!(
        control_values_after_duplicate["value_1"],
        json!({ "type": "float", "representation": "f32", "value": 12.0 })
    );
    assert!(no_second_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_node_create_base_revision_conflict_rejects_without_broadcast() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "node-create-conflict-1",
        "node-create-conflict-key",
        node_create_payload("0", "osc_conflict", "osc~ 220"),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
    let project = loaded_project_json(&runtime);

    assert_eq!(ack["payload"]["status"], "conflict");
    assert_eq!(ack["payload"]["accepted"], false);
    assert_eq!(ack["payload"]["applied"], false);
    assert_eq!(ack["payload"]["conflict"], true);
    assert_eq!(
        ack["payload"]["diagnostics"][0]["code"],
        "graph.command.target-revision-conflict"
    );
    assert_eq!(
        project["graph"]["nodes"]
            .as_array()
            .expect("nodes should be array")
            .len(),
        2
    );
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_legacy_object_commands_are_rejected_as_unsupported() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    for (index, kind) in ["object.resolve", "object.create", "object.replace"]
        .into_iter()
        .enumerate()
    {
        send_graph_command(
            &mut client_a,
            &format!("legacy-object-{index}"),
            &format!("legacy-object-key-{index}"),
            legacy_object_command_payload(kind),
        )
        .await;
        let ack = next_type(&mut client_a, "graph.ack").await;
        let supported_kinds = ack["payload"]["diagnostics"][0]["details"]["supportedKinds"]
            .as_array()
            .expect("unsupported kind diagnostic should include supportedKinds");

        assert_eq!(ack["payload"]["status"], "rejected", "{kind}");
        assert_eq!(ack["payload"]["accepted"], false, "{kind}");
        assert_eq!(
            ack["payload"]["diagnostics"][0]["code"], "graph.command.kind-unsupported",
            "{kind}"
        );
        assert!(
            supported_kinds
                .iter()
                .any(|value| value.as_str() == Some("node.create"))
        );
        assert!(
            !supported_kinds
                .iter()
                .any(|value| value.as_str() == Some(kind))
        );
    }

    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn object_text_commands_do_not_add_http_endpoints() {
    for path in [
        "/v0/sessions/default/object/resolve",
        "/v0/sessions/default/object/create",
        "/v0/sessions/default/object/replace",
    ] {
        let app = runtime_router_with_state(RuntimeServerState::default());
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "objectText": "osc~ 220" }).to_string()))
                    .expect("request builds"),
            )
            .await
            .expect("request succeeds");
        assert_eq!(response.status(), 404, "{path} should not exist");
    }
}

#[tokio::test]
async fn realtime_graph_duplicate_idempotency_key_replays_without_second_apply_or_broadcast() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "graph-once",
        "graph-dedupe-key",
        view_patch_payload(1, 140.0, 112.0),
    )
    .await;
    let first_ack = next_type(&mut client_a, "graph.ack").await;
    let client_a_echo = next_type(&mut client_a, "graph.applied").await;
    let _first_broadcast = next_type(&mut client_b, "graph.applied").await;

    send_graph_command(
        &mut client_a,
        "graph-duplicate",
        "graph-dedupe-key",
        view_patch_payload(1, 220.0, 160.0),
    )
    .await;
    let duplicate_ack = next_type(&mut client_a, "graph.ack").await;
    let duplicate_local_result = next_type(&mut client_a, "graph.applied").await;
    let no_second_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_ne!(duplicate_ack["messageId"], first_ack["messageId"]);
    assert_eq!(duplicate_ack["payload"]["accepted"], true);
    assert_eq!(duplicate_ack["payload"]["cached"], true);
    assert_eq!(
        duplicate_ack["payload"]["eventCursor"],
        first_ack["payload"]["eventCursor"]
    );
    assert_eq!(duplicate_ack["payload"]["viewRevision"], 2);
    assert!(first_ack["payload"].get("history").is_none());
    assert!(duplicate_ack["payload"].get("history").is_none());
    assert_eq!(
        duplicate_ack["payload"]["historySummary"],
        first_ack["payload"]["historySummary"]
    );
    assert!(duplicate_ack["payload"]["historySummary"]["latestEntryId"].is_string());
    assert_eq!(duplicate_local_result, client_a_echo);
    assert!(no_second_broadcast.is_err());
}

#[tokio::test]
async fn realtime_graph_unsupported_command_kind_returns_rejected_ack() {
    let runtime = spawn_loaded_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_graph_command(
        &mut client_a,
        "graph-unsupported-1",
        "graph-unsupported-key",
        json!({
            "kind": "collaboration.changeSet",
            "baseSessionRevision": 1,
            "baseGraphRevision": "1",
            "baseViewRevision": 1,
            "target": root_target("1")
        }),
    )
    .await;
    let ack = next_type(&mut client_a, "graph.ack").await;
    let no_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_eq!(ack["payload"]["status"], "rejected");
    assert_eq!(ack["payload"]["accepted"], false);
    assert_eq!(
        ack["payload"]["diagnostics"][0]["code"],
        "graph.command.kind-unsupported"
    );
    assert!(no_broadcast.is_err());
}

#[tokio::test]
async fn two_clients_receive_presence_broadcast() {
    let runtime = spawn_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let attached_a = attach(&mut client_a, "hello-a", None).await;
    let attached_b = attach(&mut client_b, "hello-b", None).await;

    assert_eq!(attached_a["type"], "session.attached");
    assert_eq!(attached_b["type"], "session.attached");

    send_presence(&mut client_a, "presence-a-1", "presence-key-a").await;
    let ack = next_type(&mut client_a, "command.ack").await;
    let broadcast = next_type(&mut client_b, "presence.updated").await;

    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(broadcast["clientId"], attached_a["clientId"]);
    assert_eq!(broadcast["windowId"], attached_a["windowId"]);
    assert_eq!(
        broadcast["payload"]["presence"]["presence"]["state"],
        "active"
    );
    assert!(broadcast["cursor"].as_str().is_some());
}

#[tokio::test]
async fn reconnect_with_in_window_cursor_replays_missed_event() {
    let runtime = spawn_runtime().await;
    let mut producer = connect_session(&runtime, "default").await;
    let attached = attach(&mut producer, "hello-producer", None).await;
    let initial_cursor = attached["payload"]["globalCursor"]
        .as_str()
        .expect("attached includes cursor")
        .to_owned();

    send_presence(&mut producer, "presence-replay", "presence-key-replay").await;
    let _ack = next_type(&mut producer, "command.ack").await;
    let produced = next_type(&mut producer, "presence.updated").await;
    producer
        .close(None)
        .await
        .unwrap_or_else(|error: TungsteniteError| panic!("producer closes: {error}"));

    let mut reconnect = connect_session(&runtime, "default").await;
    let attached_reconnect = attach(&mut reconnect, "hello-reconnect", Some(&initial_cursor)).await;
    let replayed = next_type(&mut reconnect, "presence.updated").await;

    assert_eq!(attached_reconnect["type"], "session.attached");
    assert_eq!(replayed["cursor"], produced["cursor"]);
    assert_eq!(replayed["payload"]["replayed"], true);
}

#[tokio::test]
async fn reconnect_with_unknown_cursor_receives_sync_required() {
    let runtime = spawn_runtime().await;
    let mut socket = connect_session(&runtime, "default").await;
    let attached = attach(&mut socket, "hello-initial", None).await;
    let cursor = attached["payload"]["globalCursor"]
        .as_str()
        .expect("attached includes cursor");
    let (incarnation, _) = cursor.rsplit_once(':').expect("cursor has sequence");
    socket
        .close(None)
        .await
        .unwrap_or_else(|error: TungsteniteError| panic!("socket closes: {error}"));

    let mut reconnect = connect_session(&runtime, "default").await;
    let sync = attach(
        &mut reconnect,
        "hello-stale",
        Some(&format!("{incarnation}:999")),
    )
    .await;

    assert_eq!(sync["type"], "session.syncRequired");
    assert_eq!(
        sync["payload"]["diagnostic"]["code"],
        "realtime.cursor.unknown"
    );
    assert!(sync["payload"]["snapshot"].is_object());
}

#[tokio::test]
async fn duplicate_idempotency_key_returns_cached_ack_without_second_broadcast() {
    let runtime = spawn_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let _attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;

    send_presence(&mut client_a, "presence-once", "dedupe-key").await;
    let first_ack = next_type(&mut client_a, "command.ack").await;
    let _client_a_echo = next_type(&mut client_a, "presence.updated").await;
    let first_broadcast = next_type(&mut client_b, "presence.updated").await;

    send_presence(&mut client_a, "presence-duplicate", "dedupe-key").await;
    let duplicate_ack = next_type(&mut client_a, "command.ack").await;
    let no_second_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_ne!(duplicate_ack["messageId"], first_ack["messageId"]);
    assert_eq!(duplicate_ack["connectionId"], first_ack["connectionId"]);
    assert_eq!(duplicate_ack["clientId"], first_ack["clientId"]);
    assert_eq!(duplicate_ack["windowId"], first_ack["windowId"]);
    assert_eq!(duplicate_ack["payload"]["accepted"], true);
    assert_eq!(duplicate_ack["payload"]["cached"], true);
    assert_eq!(
        duplicate_ack["payload"]["eventCursor"],
        first_ack["payload"]["eventCursor"]
    );
    assert!(first_broadcast["cursor"].as_str().is_some());
    assert!(no_second_broadcast.is_err());
}

#[tokio::test]
async fn reconnect_with_valid_resume_token_retains_identity_and_idempotency_window() {
    let runtime = spawn_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;
    let resume_token = attached_a["payload"]["resumeToken"]
        .as_str()
        .expect("attached includes resume token")
        .to_owned();

    send_presence(
        &mut client_a,
        "presence-before-reconnect",
        "resume-dedupe-key",
    )
    .await;
    let first_ack = next_type(&mut client_a, "command.ack").await;
    let _client_a_echo = next_type(&mut client_a, "presence.updated").await;
    let first_broadcast = next_type(&mut client_b, "presence.updated").await;
    let current_cursor = first_broadcast["cursor"]
        .as_str()
        .expect("presence event includes cursor")
        .to_owned();
    client_a
        .close(None)
        .await
        .unwrap_or_else(|error: TungsteniteError| panic!("client closes: {error}"));

    let mut resumed = connect_session(&runtime, "default").await;
    let resumed_attached = attach_with_resume(
        &mut resumed,
        "hello-resume",
        Some(&current_cursor),
        Some(&resume_token),
    )
    .await;

    assert_eq!(resumed_attached["type"], "session.attached");
    assert_eq!(resumed_attached["clientId"], attached_a["clientId"]);
    assert_eq!(resumed_attached["windowId"], attached_a["windowId"]);
    assert_ne!(resumed_attached["connectionId"], attached_a["connectionId"]);
    assert_ne!(
        resumed_attached["payload"]["resumeToken"],
        attached_a["payload"]["resumeToken"]
    );

    send_presence(
        &mut resumed,
        "presence-after-reconnect",
        "resume-dedupe-key",
    )
    .await;
    let resumed_ack = next_type(&mut resumed, "command.ack").await;
    let no_second_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_eq!(
        resumed_ack["connectionId"],
        resumed_attached["connectionId"]
    );
    assert_eq!(resumed_ack["clientId"], attached_a["clientId"]);
    assert_eq!(resumed_ack["windowId"], attached_a["windowId"]);
    assert_eq!(resumed_ack["payload"]["accepted"], true);
    assert_eq!(resumed_ack["payload"]["cached"], true);
    assert_eq!(
        resumed_ack["payload"]["eventCursor"],
        first_ack["payload"]["eventCursor"]
    );
    assert!(no_second_broadcast.is_err());
}

#[tokio::test]
async fn guessed_adjacent_resume_token_cannot_reuse_identity_or_idempotency_scope() {
    let runtime = spawn_runtime().await;
    let mut client_a = connect_session(&runtime, "default").await;
    let mut client_b = connect_session(&runtime, "default").await;
    let attached_a = attach(&mut client_a, "hello-a", None).await;
    let _attached_b = attach(&mut client_b, "hello-b", None).await;
    let cursor = attached_a["payload"]["globalCursor"]
        .as_str()
        .expect("attached includes cursor");
    let (incarnation, _) = cursor.rsplit_once(':').expect("cursor has sequence");
    let guessed_resume_token = format!("{incarnation}:resume:000001");

    send_presence(&mut client_a, "presence-before-guess", "guessed-dedupe-key").await;
    let first_ack = next_type(&mut client_a, "command.ack").await;
    let _client_a_echo = next_type(&mut client_a, "presence.updated").await;
    let _first_broadcast = next_type(&mut client_b, "presence.updated").await;

    let mut guessed = connect_session(&runtime, "default").await;
    let sync = attach_with_resume(
        &mut guessed,
        "hello-guessed-token",
        None,
        Some(&guessed_resume_token),
    )
    .await;

    assert_eq!(sync["type"], "session.syncRequired");
    assert_eq!(
        sync["payload"]["diagnostic"]["code"],
        "realtime.resume-token.invalid"
    );
    assert_ne!(sync["clientId"], attached_a["clientId"]);
    assert_ne!(sync["windowId"], attached_a["windowId"]);
    assert_ne!(sync["payload"]["resumeToken"], guessed_resume_token);

    send_presence(
        &mut guessed,
        "presence-after-guessed-token",
        "guessed-dedupe-key",
    )
    .await;
    let guessed_ack = next_type(&mut guessed, "command.ack").await;

    assert_eq!(guessed_ack["clientId"], sync["clientId"]);
    assert_eq!(guessed_ack["windowId"], sync["windowId"]);
    assert_ne!(guessed_ack["clientId"], first_ack["clientId"]);
    assert_ne!(guessed_ack["windowId"], first_ack["windowId"]);
    assert_eq!(guessed_ack["payload"]["accepted"], true);
    assert_eq!(guessed_ack["payload"]["cached"], false);
}

fn session_load_request(project: Value) -> Value {
    json!({
        "schema": "skenion.runtime.session-load-request",
        "schemaVersion": "0.1.0",
        "project": project,
        "mode": "loadIfEmpty",
    })
}

fn sample_project_document_current() -> Value {
    json!({
      "schema": "skenion.project",
      "schemaVersion": "0.1.0",
      "id": "minimal-value-project",
      "documentId": "20000000-0000-0000-0000-000000000001",
      "revision": "1",
      "graph": {
        "schema": "skenion.graph",
        "schemaVersion": "0.1.0",
        "id": "minimal-value",
        "revision": "1",
        "nodes": [
          {
            "id": "value_1",
            "kind": "object.core.float",
            "kindVersion": "0.1.0",
            "params": {},
            "ports": value_f32_ports_current_json()
          },
          {
            "id": "target_1",
            "kind": "object.core.float",
            "kindVersion": "0.1.0",
            "params": {},
            "ports": value_f32_ports_current_json()
          }
        ],
        "edges": [
          {
            "id": "edge_value_target",
            "source": { "nodeId": "value_1", "portId": "value" },
            "target": { "nodeId": "target_1", "portId": "cold" },
            "resolvedType": "value.core.float32"
          }
        ]
      },
      "viewState": {
        "schema": "skenion.view-state",
        "schemaVersion": "0.1.0",
        "canvas": {
          "nodes": {
            "value_1": { "x": 96.0, "y": 96.0 },
            "target_1": { "x": 260.0, "y": 96.0 }
          }
        }
      },
      "patchLibrary": [
        project_patch_definition_current_json("my-patcher"),
        project_patch_definition_current_json("add")
      ]
    })
}

fn project_patch_definition_current_json(id: &str) -> Value {
    json!({
      "id": id,
      "revision": "1",
      "metadata": { "title": id, "description": format!("{id} reusable patch") },
      "graph": {
        "schema": "skenion.graph",
        "schemaVersion": "0.1.0",
        "id": format!("{id}-graph"),
        "revision": "1",
        "nodes": [],
        "edges": []
      }
    })
}

fn project_patch_definition_with_float_interface_current_json(id: &str) -> Value {
    json!({
      "id": id,
      "revision": "2",
      "metadata": { "title": id, "description": format!("{id} reusable patch") },
      "graph": {
        "schema": "skenion.graph",
        "schemaVersion": "0.1.0",
        "id": format!("{id}-graph"),
        "revision": "2",
        "nodes": [
          {
            "id": "patch_in",
            "kind": "object.core.inlet",
            "kindVersion": "0.1.0",
            "params": { "portId": "value", "label": "Value" },
            "ports": [
              { "id": "out", "direction": "output", "type": "value.core.float32", "rate": "control" }
            ]
          },
          {
            "id": "patch_out",
            "kind": "object.core.outlet",
            "kindVersion": "0.1.0",
            "params": { "portId": "result", "label": "Result" },
            "ports": [
              { "id": "in", "direction": "input", "type": "value.core.float32", "rate": "control" }
            ]
          }
        ],
        "edges": []
      }
    })
}

fn value_f32_ports_current_json() -> Value {
    json!([
      {
        "id": "in",
        "direction": "input",
        "label": "In",
        "type": "value.core.message",
        "rate": "control",
        "required": false,
        "triggerMode": "trigger",
        "accepts": [
          "value.core.float32",
          "value.core.int32",
          "value.core.uint32",
          "value.core.bool",
          "value.core.bang"
        ],
        "messageKeys": {
          "accepted": ["bang", "set", "float", "int", "uint", "bool"],
          "silent": ["set"],
          "trigger": ["bang", "float", "int", "uint", "bool"],
          "store": ["set", "float", "int", "uint", "bool"],
          "emit": ["bang", "float", "int", "uint", "bool"]
        }
      },
      {
        "id": "cold",
        "direction": "input",
        "label": "Cold",
        "type": "value.core.float32",
        "rate": "control",
        "required": false,
        "triggerMode": "passive"
      },
      {
        "id": "value",
        "direction": "output",
        "label": "Value",
        "type": "value.core.float32",
        "rate": "control"
      }
    ])
}
