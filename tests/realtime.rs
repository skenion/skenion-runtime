use std::{net::SocketAddr, time::Duration};

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, header},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
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
    let app = runtime_router_with_state(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("runtime serves");
    });
    TestRuntime { addr, handle }
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
                .body(Body::from(sample_project_document_current().to_string()))
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

async fn send_presence(socket: &mut TestSocket, message_id: &str, idempotency_key: &str) {
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
                "presence": {
                    "state": "active",
                    "selection": { "nodeIds": ["value_1"] }
                }
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
    send_json(
        socket,
        json!({
            "schema": "skenion.runtime.realtime",
            "schemaVersion": "0.1.0",
            "type": "control.command",
            "messageId": message_id,
            "sessionId": "default",
            "commandId": message_id,
            "correlationId": message_id,
            "idempotencyKey": idempotency_key,
            "payload": {
                "nodeId": node_id,
                "portId": port_id,
                "message": message
            }
        }),
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
        "kind": "collaboration.changeSet",
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
    let ack = next_type(&mut client_a, "control.ack").await;
    let broadcast = next_type(&mut client_b, "control.emitted").await;

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(ack["payload"]["commandId"], "control-bang-1");
    assert_eq!(ack["payload"]["correlationId"], "control-bang-1");
    assert_eq!(ack["payload"]["idempotencyKey"], "control-bang-key");
    assert!(ack["payload"]["controlSequence"].as_u64().is_some());
    assert!(ack["payload"]["controlRevision"].as_u64().is_some());
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
    let ack = next_type(&mut client_a, "control.ack").await;
    let broadcast = next_type(&mut client_b, "control.emitted").await;

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["accepted"], true);
    assert_eq!(ack["payload"]["changed"], true);
    assert_eq!(ack["payload"]["controlRevision"], 1);
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
    let ack = next_type(&mut client_a, "control.ack").await;
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
    let first_ack = next_type(&mut client_a, "control.ack").await;
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
    let duplicate_ack = next_type(&mut client_a, "control.ack").await;
    let duplicate_local_result = next_type(&mut client_a, "control.emitted").await;
    let no_second_broadcast = timeout(Duration::from_millis(200), next_json(&mut client_b)).await;

    assert_ne!(duplicate_ack["messageId"], first_ack["messageId"]);
    assert_eq!(duplicate_ack["payload"]["accepted"], true);
    assert_eq!(duplicate_ack["payload"]["cached"], true);
    assert_eq!(
        duplicate_ack["payload"]["eventCursor"],
        first_ack["payload"]["eventCursor"]
    );
    assert_eq!(duplicate_ack["payload"]["controlRevision"], 1);
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
    let control_ack = next_type(&mut client_a, "control.ack").await;
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
    let ack = next_type(&mut client_a, "control.ack").await;
    let broadcast = next_type(&mut client_b, "control.emitted").await;

    assert_eq!(ack["payload"]["status"], "accepted");
    assert_eq!(ack["payload"]["changed"], true);
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
    assert_eq!(ack["payload"]["kind"], "collaboration.changeSet");
    assert_eq!(ack["payload"]["baseGraphRevision"], "1");
    assert_eq!(ack["payload"]["graphRevision"], "2");
    assert_eq!(ack["payload"]["viewRevision"], 2);
    assert!(ack["payload"].get("history").is_none());
    assert!(ack["payload"]["historySummary"]["latestEntryId"].is_string());
    assert_eq!(ack["payload"]["historySummary"]["canUndo"], true);
    assert_eq!(ack["payload"]["historySummary"]["canRedo"], false);
    assert_eq!(broadcast["payload"]["kind"], "collaboration.changeSet");
    assert_eq!(broadcast["payload"]["graphRevision"], "2");
    assert!(broadcast["payload"]["historyEntryId"].is_string());
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
            "kind": "history.undo",
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

fn sample_project_document_current() -> Value {
    json!({
      "schema": "skenion.project",
      "schemaVersion": "0.1.0",
      "id": "minimal-value-project",
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
      "patchLibrary": [],
      "nodes": [
        {
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.float",
          "version": "0.1.0",
          "displayName": "Float",
          "category": "Typed Controls",
          "ports": value_f32_ports_current_json(),
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": ["value.core.float32.v0.1"]
        }
      ]
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
