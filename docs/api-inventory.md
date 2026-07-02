# Runtime API Inventory

This file is the concrete Runtime API inventory used to clean up Studio.
It is intentionally Runtime-owned. Contracts may define shared DTO shapes, but
Runtime owns routes, WebSocket frames, session state, graph mutation, object
resolution, catalog projection, control input, and issues.

Source files checked for this inventory:

- `src/server.rs`
- `src/realtime.rs`
- `src/session.rs`
- `src/control_state.rs`
- `src/object_spec.rs`

## Hard Rules For Studio

1. `GET /v0/sessions/{session_id}` is the WebSocket live endpoint.
   A plain HTTP request to this route returns HTTP 426. Studio must not use
   `fetch()` here to read a session.
2. Live Studio hydration comes from the WebSocket `session.attached` or
   `session.syncRequired` frame payload, specifically `payload.snapshot`.
3. HTTP `snapshot` and HTTP `node-catalog` are read-only debug, CLI, or
   fallback surfaces. They are not the normal live Studio path.
4. Live graph edits go through WebSocket `graph.command`.
5. Runtime validates and applies node operations. Studio must not create a
   materialized live node locally and then ask Runtime to accept it.
6. Runtime-owned node catalog answers "what can be created/resolved now"; it is
   not invalidated by ordinary graph edge edits, node positions, selection,
   cursor, history, or transient `node.input`.

## HTTP Route Inventory

Legend:

- "Creates session" means the handler calls `state.sessions.get_or_create`.
- "Graph mutation" means it can change persisted loaded graph/session graph
  state.
- "Live Studio use" is the intended cleanup decision.

| Method | Path | Handler | Request body | Response type | State effect | Live Studio use |
| --- | --- | --- | --- | --- | --- | --- |
| `GET` | `/health` | `health` | none | `HealthResponse` | none | OK for process ping. Not session readiness. |
| `GET` | `/v0/runtime/info` | `runtime_info` | none | `RuntimeInfoResponse` | none | Required for capability/API gating. |
| `GET` | `/v0/sidecar/startup` | `sidecar_startup` | none | `RuntimeSidecarStartupResponse` | none | Desktop/local shell only. |
| `GET` | `/v0/sidecar/health` | `sidecar_health` | none | `RuntimeSidecarHealthResponse` | none | Desktop/local shell only. |
| `POST` | `/v0/sidecar/shutdown` | `sidecar_shutdown` | raw bytes parsed by sidecar helper | `RuntimeSidecarShutdownResponse` | records runtime issues | Desktop/local shell explicit shutdown only. |
| `GET` | `/v0/extensions` | `runtime_extensions` | none | `RuntimeExtensionListResponse` | none | Package/extension panel snapshot. Do not poll for live sync. |
| `GET` | `/v0/packages` | `runtime_packages` | none | `PackageRegistryListResponseV01` | none | Package panel snapshot. Do not poll for live sync. |
| `GET` | `/v0/runtime/logs` | `runtime_logs` | none | `RuntimeLogSnapshotResponse` | none | Logs panel snapshot. |
| `GET` | `/v0/runtime/logs/stream` | `runtime_logs_stream` | none | SSE stream | stream subscription | Logs panel stream until WS log surface exists. |
| `GET` | `/v0/io/devices` | `io_devices` | none | `RuntimeIoDeviceListResponse` | records IO issues into logs | Explicit device refresh only. Do not background-poll. |
| `POST` | `/v0/validate` | `validate_project_endpoint` | project JSON | `RuntimeApiResponse` | no session graph mutation | Import/debug utility only. |
| `POST` | `/v0/plan` | `plan_project_endpoint` | project JSON | `RuntimeApiResponse` | no session graph mutation | Debug utility only. |
| `POST` | `/v0/run` | `run_project_endpoint` | project JSON plus optional frames | `RuntimeApiResponse` | no session graph mutation | Prototype/debug utility, not live Studio path. |
| `GET` | `/v0/sessions/{session_id}` | `realtime_session_by_id` | WebSocket upgrade | WS frames or HTTP 426 body | creates session record only after a valid WebSocket upgrade | Primary live path only as WebSocket. Never plain HTTP. |
| `DELETE` | `/v0/sessions/{session_id}` | `clear_session_by_id` | none | `RuntimeSessionResponse` | clears session | Explicit destructive command only. |
| `GET` | `/v0/sessions/{session_id}/info` | `session_info_by_id` | none | `RuntimeSessionInfoResponse` | creates session record | Settings/debug/fallback. |
| `GET` | `/v0/sessions/{session_id}/snapshot` | `session_snapshot_by_id` | none | `RuntimeSessionResponse` | creates session record, no graph mutation | Debug/CLI/fallback. Not live hydration. |
| `GET` | `/v0/sessions/{session_id}/node-catalog` | `session_node_catalog_by_id` | none | `NodeCatalogSnapshotV01` or HTTP 404 issue | no mutation; absent sessions return 404 | Debug/CLI/fallback. Not normal live hydration. |
| `GET` | `/v0/sessions/{session_id}/events/stream` | `disabled_session_events_stream_by_id` | `server::tests::legacy_http_live_routes_return_gone_with_ws_replacements` | HTTP 410 `skenion.runtime.http-live-channel-disabled` | none | Disabled legacy live channel. Use WS `session.hello` replay/resume. |
| `POST` | `/v0/sessions/{session_id}/load` | `load_session_by_id` | project JSON | `RuntimeSessionResponse` | replaces loaded session project | Open/import/load only. |
| `POST` | `/v0/sessions/{session_id}/validate` | `validate_session_by_id` | none | `RuntimeSessionResponse` | write lock, no graph mutation | Explicit validate command. |
| `POST` | `/v0/sessions/{session_id}/plan` | `plan_session_by_id` | none | `RuntimeSessionResponse` | write lock, no graph mutation | Explicit plan command. |
| `POST` | `/v0/sessions/{session_id}/run` | `run_session_by_id` | `SessionRunRequest` | `RuntimeSessionResponse` | write lock, no graph mutation expected | Explicit run/debug command. |
| `POST` | `/v0/sessions/{session_id}/mutate` | `disabled_session_mutate_by_id` | `server::tests::legacy_http_live_routes_return_gone_with_ws_replacements` | HTTP 410 `skenion.runtime.http-live-channel-disabled` | none | Disabled legacy live mutation. Use WS `graph.command`. |
| `POST` | `/v0/sessions/{session_id}/operation` | `disabled_session_operation_by_id` | `server::tests::legacy_http_live_routes_return_gone_with_ws_replacements` | HTTP 410 `skenion.runtime.http-live-channel-disabled` | none | Disabled legacy paste mutation. Use WS `graph.command` kind `graph.pasteFragment`. |
| `POST` | `/v0/sessions/{session_id}/operations` | `disabled_session_operations_by_id` | `server::tests::legacy_http_live_routes_return_gone_with_ws_replacements` | HTTP 410 `skenion.runtime.http-live-channel-disabled` | none | Disabled legacy collaboration mutation multiplexer. Use WS `graph.command`. |
| `POST` | `/v0/sessions/{session_id}/collaboration/presence` | `disabled_session_collaboration_presence_by_id` | `server::tests::legacy_http_live_routes_return_gone_with_ws_replacements` | HTTP 410 `skenion.runtime.http-live-channel-disabled` | none | Disabled legacy presence mutation. Current realtime surface has no presence frame. |
| `POST` | `/v0/sessions/{session_id}/collaboration/selection` | `disabled_session_collaboration_selection_by_id` | `server::tests::legacy_http_live_routes_return_gone_with_ws_replacements` | HTTP 410 `skenion.runtime.http-live-channel-disabled` | none | Disabled legacy selection mutation. Use WS `selection.update`. |
| `GET` | `/v0/sessions/{session_id}/collaboration/events/stream` | `disabled_session_collaboration_events_stream_by_id` | `server::tests::legacy_http_live_routes_return_gone_with_ws_replacements` | HTTP 410 `skenion.runtime.http-live-channel-disabled` | none | Disabled legacy collaboration stream. Use WS realtime events. |
| `GET` | `/v0/sessions/{session_id}/history` | `session_history_by_id` | none | `RuntimeHistory` | no graph mutation | History panel only. Do not drive graph repaint from this. |
| `POST` | `/v0/sessions/{session_id}/undo` | `disabled_session_undo_by_id` | `server::tests::legacy_http_live_routes_return_gone_with_ws_replacements` | HTTP 410 `skenion.runtime.http-live-channel-disabled` | none | Disabled legacy history mutation. Use WS `graph.command` kind `history.undo`. |
| `POST` | `/v0/sessions/{session_id}/redo` | `disabled_session_redo_by_id` | `server::tests::legacy_http_live_routes_return_gone_with_ws_replacements` | HTTP 410 `skenion.runtime.http-live-channel-disabled` | none | Disabled legacy history mutation. Use WS `graph.command` kind `history.redo`. |
| `POST` | `/v0/sessions/{session_id}/control/event` | `disabled_session_control_event_by_id` | `server::tests::legacy_http_live_routes_return_gone_with_ws_replacements` | HTTP 410 `skenion.runtime.http-live-channel-disabled` | none | Disabled legacy control mutation. Use WS `node.input`. |
| `GET` | `/v0/sessions/{session_id}/control/state` | `control_state_by_id` | none | `RuntimeControlStateResponse` | no graph mutation | Inspector/debug/fallback snapshot. |
| `POST` | `/v0/sessions/{session_id}/control/read` | `control_read_by_id` | `RuntimeControlReadRequest` | `RuntimeControlReadResponse` | no graph mutation | Inspector/debug read. |
| `GET` | `/v0/sessions/{session_id}/preview` | `preview_status_by_id` | none | `RuntimePreviewStatusResponse` | preview manager read/update under lock | Preview panel. |
| `POST` | `/v0/sessions/{session_id}/preview/start` | `start_preview_by_id` | optional `RuntimePreviewStartRequest` bytes | `RuntimePreviewStatusResponse` | preview process/state mutation | Explicit preview start. |
| `POST` | `/v0/sessions/{session_id}/preview/stop` | `stop_preview_by_id` | none | `RuntimePreviewStatusResponse` | preview process/state mutation | Explicit preview stop. |
| `POST` | `/v0/sessions/{session_id}/preview/restart` | `restart_preview_by_id` | none | `RuntimePreviewStatusResponse` | preview process/state mutation | Explicit preview restart. |
| `GET` | `/v0/sessions/{session_id}/render/generated-shader` | `generated_shader_by_id` | none | `GeneratedShaderResponse` | no graph mutation | Debug/inspector. |
| `GET` | `/v0/sessions/{session_id}/telemetry` | `session_telemetry_by_id` | none | `RuntimeTelemetrySnapshot` | no graph mutation | Performance/debug panel. |
| `GET` | `/v0/sessions/{session_id}/telemetry/stream` | `session_telemetry_stream_by_id` | none | SSE stream | stream subscription | Performance/debug stream. |
| `POST` | `/v0/assets/import` | `import_asset` | upload body, max `MAX_ASSET_UPLOAD_BYTES` | `RuntimeAssetImportResponse` | asset store mutation | Explicit asset import. |
| `GET` | `/v0/assets` | `list_assets` | none | `RuntimeAssetListResponse` | no asset mutation | Asset browser. |
| `GET` | `/v0/assets/{asset_id}` | `get_asset` | none | `RuntimeAssetGetResponse` | no asset mutation | Asset preview/use. |

## WebSocket Endpoint

Endpoint:

```text
GET /v0/sessions/{session_id}
```

This route is valid for live use only when the request is a WebSocket upgrade.
Plain HTTP returns:

- status: `426 Upgrade Required`
- header: `Upgrade: websocket`
- schema: `skenion.runtime.realtime.upgradeRequired`
- issue code: `realtime.websocket-upgrade-required`

Plain HTTP no longer creates a session record before returning 426. Studio must
still avoid this route unless it is opening a WebSocket.

## WebSocket Envelope

Rust type: `RuntimeRealtimeEnvelope`.

Required fields in the serialized envelope:

| Field | Type | Notes |
| --- | --- | --- |
| `schema` | string | Runtime realtime schema. |
| `schemaVersion` | string | Runtime realtime schema version. |
| `type` | string | Frame discriminator. |
| `messageId` | string | Client/server frame id. |
| `sessionId` | string | Must match the URL session id. |
| `payload` | object | Frame-specific payload. |

Optional fields:

| Field | Type | Notes |
| --- | --- | --- |
| `connectionId` | string | Issued by Runtime after attach. |
| `clientId` | string | Client identity. |
| `windowId` | string | Window identity. |
| `commandId` | string | Stable command id for UI command tracking. |
| `correlationId` | string | Reply correlation. |
| `idempotencyKey` | string | Required for command frames listed below. |
| `sequence` | number | Server event sequence. |
| `cursor` | string | Server replay cursor or client last cursor. |
| `createdAt` | string | Runtime-created timestamp string. |

## WebSocket Inbound Frames

| Type | Handler path | Required before use | Required payload/key fields | State effect | Studio rule |
| --- | --- | --- | --- | --- | --- |
| `session.hello` | attach/resume branch in `handle_runtime_realtime_socket` | none | optional `resumeToken`, `lastCursor`, `nodeCatalog` | creates/renews connection identity, may replay events | First frame after opening socket. |
| `selection.update` | `handle_selection_update` | `session.hello` | `idempotencyKey`, `target`, `selection`, optional `cursor`, `ttlMs` | collaboration selection mutation | Live selection path. Runtime derives session/participant/timestamps. |
| `graph.command` | `handle_graph_command` | `session.hello` | `idempotencyKey`, `payload.kind` | graph/view/node mutation | Primary live graph authoring path. |
| `node.input` | `handle_node_input` | `session.hello` | `commandId`, `idempotencyKey`, ordered `payload.inputs[]` | transient control input | Top-level live execution input. Applies inputs in order. |
| `nodeCatalog.request` | `handle_node_catalog_request` | `session.hello` | optional `knownRevision` | no mutation | Request catalog snapshot only after `nodeCatalog.changed` or explicit refresh. |

If any client frame has a `sessionId` different from the URL session id, Runtime
returns `runtime.issue` with code `realtime.session.mismatch`.

If a command frame is sent before `session.hello`, Runtime returns
`runtime.issue` with code `realtime.session.not-attached`.

## WebSocket Outbound Frames

| Type | Produced by | Payload contains | Who should consume it | Studio rule |
| --- | --- | --- | --- | --- |
| `session.attached` | successful `session.hello` | `connectionId`, `clientId`, `windowId`, `resumeToken`, `currentRevisions`, `snapshot`, `globalCursor`, `nodeCatalog` | attaching client | Hydrate live graph from `payload.snapshot`. |
| `session.syncRequired` | failed resume/replay gap | same as attach plus `issue` | attaching client | Replace local state from snapshot; do not patch over stale local state. |
| `selection.updated` | selection handling and replay | selection envelope plus TTL/replay metadata | all live clients | Selection UI only. |
| `command.ack` | command handlers | accepted/rejected/conflict payload, command kind, revisions, node/operation result, issues | sender | Sender feedback only. Do not treat as final multi-client graph sync. |
| `graph.applied` | accepted `graph.command` | kind, target, node result, operation result, revisions, issues | all live clients | Authoritative graph update event. |
| `control.emitted` | successful `node.input` with emitted events or changed values | `events[]`, `issues[]`, control sequence/revision, changed values | live clients; duplicate sender replay only on idempotency hit | Authoritative transient control feedback. |
| `nodeCatalog.snapshot` | `nodeCatalog.request` | status `included`, `catalogRevision`, `snapshot` | requester | Replace cached catalog if revision differs. |
| `nodeCatalog.unchanged` | `nodeCatalog.request` | status `unchanged`, `catalogRevision` | requester | Keep cached catalog. |
| `nodeCatalog.changed` | graph command when catalog projection changed | new catalog revision and snapshot metadata | all live clients | Request or apply catalog refresh. Must not fire for ordinary edge/node usage edits. |
| `runtime.issue` | protocol/runtime issue | `issue.code`, `issue.message`, `issue.details` | triggering client | Show scoped issue; do not wipe graph. |

## `session.hello.payload.nodeCatalog`

Studio can request initial catalog hydration in the first WS frame:

```json
{
  "nodeCatalog": {
    "mode": "none | ifChanged | always",
    "knownRevision": {
      "algorithm": "sha256",
      "value": "..."
    }
  }
}
```

Runtime behavior:

| Mode | Matching known revision | Response payload |
| --- | --- | --- |
| `none` or omitted | any | `nodeCatalog.status = "notRequested"` |
| `ifChanged` | yes | `nodeCatalog.status = "unchanged"` with no snapshot |
| `ifChanged` | no | `nodeCatalog.status = "included"` with snapshot |
| `always` | any | `nodeCatalog.status = "included"` with snapshot |

## `nodeCatalog.request`

Payload:

```json
{
  "knownRevision": {
    "algorithm": "sha256",
    "value": "..."
  }
}
```

Response:

- `nodeCatalog.unchanged` if the supplied revision matches.
- `nodeCatalog.snapshot` otherwise.

This request does not scan packages, append logs, mutate session graph state, or
change the catalog revision. The HTTP node catalog endpoint and WS catalog
frames must use the same cached projection.

## `graph.command` Payload

Rust struct: `GraphCommandPayload`.

Common fields:

| Field | Type | Used by |
| --- | --- | --- |
| `kind` | string | all graph commands |
| `baseSessionRevision` | number | conflict check |
| `baseGraphRevision` | string | conflict check |
| `baseViewRevision` | number | view conflict check |
| `target` | `GraphTargetRef` | commands that address graph/patch target |
| `description` | string | history/debug description |
| `surfacePath` | JSON | carried in ack/applied payload |

Node/object fields:

| Field | Type | Used by |
| --- | --- | --- |
| `objectSpec` | string | `node.resolve`, `node.create`, `node.replace` |
| `nodeId` | string | `node.replace`, `node.delete`, `node.update` |
| `requestedNodeId` | string | `node.create` |
| `view` | `CanvasNodeView` | `node.create`, `node.replace` |
| `params` | object | `node.create`, `node.update` |
| `request` | `PasteGraphFragmentRequest` | `graph.pasteFragment` |
| `scope` | `client` or `global` | `history.undo`, `history.redo`; defaults to `client` |
| `unresolvedPolicy` | `reject` or `materialize-issue` | object spec materialization |
| `interfaceIncidentEdgePolicy` | Contracts enum | `node.replace` |

View/collaboration fields:

| Field | Type | Used by |
| --- | --- | --- |
| `viewPatch` | `RuntimeViewPatch` | `view.patch` |
| `changes` | `RuntimeCollaborationChange[]` | `graph.changeSet` |

## `graph.command.payload.kind`

| Kind | Required fields | Applies persisted graph mutation | Control mutation | Success events |
| --- | --- | --- | --- | --- |
| `view.patch` | `viewPatch`; optional `target`; revision fields if supplied must match | yes | no | `command.ack`, `graph.applied` |
| `graph.changeSet` | `target`, non-empty `changes` | yes | no | `command.ack`, `graph.applied` |
| `graph.pasteFragment` | `request` | yes | no | `command.ack`, `graph.applied`; paste response appears under `payload.operation` |
| `history.undo` | optional `scope` | yes if history entry is available | no | `command.ack`, `graph.applied` |
| `history.redo` | optional `scope` | yes if history entry is available | no | `command.ack`, `graph.applied` |
| `node.resolve` | `objectSpec`; valid target if supplied | no | no | `command.ack` only |
| `node.create` | `objectSpec`; optional `requestedNodeId`, `view`, `params`, target | yes if resolved/materialized | no | `command.ack`, `graph.applied`; may emit `nodeCatalog.changed` if catalog projection changes |
| `node.replace` | `objectSpec`, `nodeId`; optional `interfaceIncidentEdgePolicy` | yes if resolved/materialized | no | `command.ack`, `graph.applied`; `node.droppedEdgeIds` in result when interface changed |
| `node.delete` | `nodeId` | yes | no | `command.ack`, `graph.applied`; `node.droppedEdgeIds` for incident edges |
| `node.update` | `nodeId`, non-empty `params` | yes | no | `command.ack`, `graph.applied` |

Unsupported kinds return issue code `graph.command.kind-unsupported`.
Supported kinds are exactly:

```text
view.patch
graph.changeSet
graph.pasteFragment
node.resolve
node.create
node.replace
node.delete
node.update
history.undo
history.redo
```

Old draft object command names such as `object.resolve`, `object.create`,
`object.replace`, `objectSpec.resolve`, `objectSpec.create`, and
`objectSpec.replace` are not supported live command kinds.

## Node Command Results

`command.ack.payload.node` and `graph.applied.payload.node` are Runtime-owned
node command result payloads. They are not Studio-local node definitions.

Current result facts from `src/realtime.rs`:

- `node.resolve` returns `applied: false`.
- `node.create`, `node.replace`, `node.delete`, and `node.update` return
  `applied: true` only after Runtime mutates the session graph.
- `node.replace` and `node.delete` can return dropped incident edge ids.
- unresolved object spec returns `node.command.unresolved` unless the unresolved
  policy materializes a issue node.

## `node.input` Payload

Rust struct: `NodeInputPayload`.

```json
{
  "inputs": [
    {
      "nodeId": "value_1",
      "portId": "in",
      "message": {
        "key": "float",
        "atoms": [{ "type": "float", "representation": "f32", "value": 1.0 }]
      }
    }
  ]
}
```

`inputs` must be non-empty and are applied in order. `message` uses the
Contracts `MessageValueV01` shape: `key` plus ordered `atoms`. Runtime does not
accept a `selector` field on the current public wire surface.

`node.input` returns sender-only `command.ack` with `applied: false` because it
is transient control input, not graph mutation. When an input emits control
events or changes values, Runtime publishes `control.emitted` with `events[]`,
`issues[]`, and control sequence/revision metadata. Duplicate `node.input`
frames with the same client/window/idempotency scope replay the cached
`command.ack` and local emitted result to the sender without reapplying input or
rebroadcasting.

## Catalog Revision Rules

The catalog revision changes only when the set of available node providers or
catalog-visible interfaces changes.

Must change catalog revision:

- first-party core object registry version changes
- package/native registry revision changes
- project patch is added, removed, renamed, or changes shortcut
- project patch inlet/outlet external interface changes
- catalog-visible patch title, description, help text, or display metadata
  changes

Must not change catalog revision:

- root graph revision changes
- edge create/delete/update
- node position/view update
- ordinary node parameter update
- selection, cursor, or presence update
- undo/redo history update
- `node.input`
- project patch internal edge change when external interface and display
  metadata are unchanged
- project patch internal implementation node change when external interface and
  display metadata are unchanged

## Studio Cleanup Decisions

Use HTTP for:

- `/health`
- `/v0/runtime/info`
- sidecar startup/health/shutdown
- runtime logs snapshot/stream
- package/extension panels
- explicit IO device refresh only
- explicit preview start/stop/restart/status
- assets import/list/get
- fallback/debug `snapshot`
- fallback/debug `node-catalog`
- fallback/debug control state/read
- fallback/debug telemetry

Use WebSocket for:

- initial live session attach and hydration
- node catalog hydration for live sessions
- node create/replace/delete/update
- object spec preview through `node.resolve`
- bang/message/slider transient input through `node.input`
- view patch/node movement
- live graph applied events
- live catalog changed events
- live selection through `selection.update`

Stop using in normal live Studio flow:

- plain HTTP `GET /v0/sessions/{session_id}`
- repeated HTTP `GET /v0/sessions/{session_id}/snapshot`
- repeated HTTP `GET /v0/sessions/{session_id}/node-catalog`
- HTTP `/mutate` for ordinary graph edits; it now returns HTTP 410
- HTTP `/operation` and `/operations` for ordinary live edits; they now return HTTP 410
- HTTP collaboration presence/selection for live collaboration; they now return HTTP 410
- session/collaboration SSE as the graph authority; these routes now return HTTP 410
- HTTP `/control/event` for bang/slider/message; it now returns HTTP 410 and points to `node.input`
- `presence.update`, `presence.updated`, `control.command`, `graph.ack`, and
  `runtime.error`; current Runtime realtime uses `selection.update`,
  `command.ack`, and `runtime.issue`

## Concrete Cleanup Issues Found

1. `GET /v0/sessions/{session_id}` is WebSocket-only and returns HTTP 426 to
   non-WebSocket callers. Studio must not probe this route with HTTP.
2. `/v0/io/devices` is not a pure read: it records IO issues into runtime
   logs. It should not be used as a polling endpoint.
3. `control.command` and HTTP `/control/event` are disabled live paths.
   Studio live control should use top-level WS `node.input`.
4. HTTP `/mutate`, `/operation`, and `/operations` are disabled live graph
   mutation surfaces. Studio live edits should use WS `graph.command`.
5. `command.ack` is sender feedback only. Studio must treat only
   `graph.applied` as multi-client graph state, not ack frames.
6. SSE session/collaboration streams are disabled for live Runtime state.
   They should be replaced by WS replay/resume for any remaining consumers or
   removed after Studio no longer depends on them.
