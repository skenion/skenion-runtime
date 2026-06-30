use super::*;

fn test_binding_format() -> EndpointBindingValueFormat {
    EndpointBindingValueFormat {
        binding_id: "edge_value_target".to_owned(),
        binding_epoch: 2,
        format_revision: 7,
        format_digest: None,
        value_format: crate::ValueFormat {
            value_type_id: "value.core.float32".to_owned(),
            format: Some("f32".to_owned()),
            shape: None,
            dynamic_shape: None,
            layout: None,
            strides: None,
            byte_length: None,
            sample_rate: None,
            channels: None,
            channel_layout: None,
            color_space: None,
            color_range: None,
            transfer: None,
            primaries: None,
            alpha_policy: None,
            resource_kind: None,
        },
        source: Some(crate::ValueEndpointRef {
            node_id: "value_1".to_owned(),
            port_id: "value".to_owned(),
        }),
        target: Some(crate::ValueEndpointRef {
            node_id: "target_1".to_owned(),
            port_id: "cold".to_owned(),
        }),
        delivery: None,
    }
}

fn test_occurrence_header() -> ValueOccurrenceHeader {
    ValueOccurrenceHeader {
        binding_id: "edge_value_target".to_owned(),
        binding_epoch: 2,
        format_revision: 7,
        sequence: 1,
        clock: None,
        timestamp: None,
        payload_kind: crate::ValuePayloadKind::Json,
        byte_length: None,
        byte_offset: None,
        actual_shape: None,
        flags: None,
        dropped_before: None,
        duration: None,
    }
}

fn realtime_event(session_id: &str, sequence: u64, cursor: &str) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: EVENT_PRESENCE_UPDATED.to_owned(),
        message_id: format!("{session_id}_presence_{sequence:06}"),
        session_id: session_id.to_owned(),
        connection_id: None,
        client_id: None,
        window_id: None,
        command_id: None,
        correlation_id: None,
        idempotency_key: None,
        sequence: Some(sequence),
        cursor: Some(cursor.to_owned()),
        created_at: Some(created_at_now()),
        payload: json!({ "replayed": false }),
    }
}

fn client_frame(
    session_id: &str,
    message_type: &str,
    message_id: &str,
    payload: Value,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: message_type.to_owned(),
        message_id: message_id.to_owned(),
        session_id: session_id.to_owned(),
        connection_id: None,
        client_id: None,
        window_id: None,
        command_id: Some(format!("{message_id}-command")),
        correlation_id: Some(format!("{message_id}-correlation")),
        idempotency_key: None,
        sequence: None,
        cursor: None,
        created_at: None,
        payload,
    }
}

fn graph_payload(payload: Value) -> GraphCommandPayload {
    serde_json::from_value(payload).expect("graph command payload should parse")
}

fn root_target(base_revision: &str) -> GraphTargetRef {
    GraphTargetRef {
        path: PatchPath::Root,
        base_revision: base_revision.to_owned(),
        target_revision: None,
    }
}

fn package_patch_target(base_revision: &str) -> GraphTargetRef {
    GraphTargetRef {
        path: PatchPath::PackagePatchDefinition {
            package_id: "pkg".to_owned(),
            patch_id: "help".to_owned(),
            version: None,
        },
        base_revision: base_revision.to_owned(),
        target_revision: None,
    }
}

fn test_identity() -> RuntimeRealtimeConnectionIdentity {
    RuntimeRealtimeConnectionIdentity {
        connection_id: "conn_test".to_owned(),
        client_id: "client_test".to_owned(),
        window_id: "window_test".to_owned(),
        resume_token: "rtresume-test".to_owned(),
    }
}

fn port_for_json(
    id: &str,
    direction: ObjectTextPortDirection,
    rate: ObjectTextPortRate,
    activation: Option<ObjectTextPortActivation>,
) -> crate::object_text::ObjectTextPort {
    crate::object_text::ObjectTextPort {
        id: id.to_owned(),
        direction,
        port_type: "value.core.float32".to_owned(),
        label: Some(format!("{id} label")),
        rate,
        accepts: None,
        activation,
        message_keys: None,
    }
}

fn resolved_float_object_text(input: &str) -> ObjectTextResolution {
    ObjectRegistry::first_party_core().resolve(input)
}

fn object_text_resolution_with_all_port_variants() -> ObjectTextResolution {
    let mut resolution = resolved_float_object_text("sig~ 440");
    resolution.instance_ports = vec![
        port_for_json(
            "event_in",
            ObjectTextPortDirection::Input,
            ObjectTextPortRate::Event,
            Some(ObjectTextPortActivation::Trigger),
        ),
        port_for_json(
            "control_in",
            ObjectTextPortDirection::Input,
            ObjectTextPortRate::Control,
            Some(ObjectTextPortActivation::Latched),
        ),
        port_for_json(
            "audio_out",
            ObjectTextPortDirection::Output,
            ObjectTextPortRate::Audio,
            Some(ObjectTextPortActivation::Passive),
        ),
        port_for_json(
            "render_out",
            ObjectTextPortDirection::Output,
            ObjectTextPortRate::Render,
            None,
        ),
        port_for_json(
            "gpu_out",
            ObjectTextPortDirection::Output,
            ObjectTextPortRate::Gpu,
            None,
        ),
        port_for_json(
            "resource_out",
            ObjectTextPortDirection::Output,
            ObjectTextPortRate::Resource,
            None,
        ),
        port_for_json(
            "io_out",
            ObjectTextPortDirection::Output,
            ObjectTextPortRate::Io,
            None,
        ),
    ];
    resolution.candidates = vec![crate::object_text::ObjectTextCandidateSummary {
        id: "object.core.sig".to_owned(),
        source: "core".to_owned(),
        kind: "object.core.sig".to_owned(),
        display_name: "Signal".to_owned(),
    }];
    resolution
        .diagnostics
        .push(crate::object_text::ObjectTextDiagnostic {
            code: "object-text.test-warning".to_owned(),
            message: "test warning".to_owned(),
        });
    resolution
}

fn assert_response_diagnostic_code(response: &RuntimePatchResponse, code: &str) {
    assert_eq!(response.diagnostics[0].code.as_deref(), Some(code));
}

#[test]
fn value_occurrence_header_guard_accepts_current_binding() {
    let binding = test_binding_format();
    let header = test_occurrence_header();
    let binding_formats = [binding.clone()];

    let accepted = validate_value_occurrence_header_for_session_binding(&header, &binding_formats)
        .expect("current binding should be accepted");

    assert_eq!(accepted, &binding);
}

#[test]
fn value_occurrence_header_guard_rejects_invalid_header() {
    let mut header = test_occurrence_header();
    header.binding_id.clear();

    let diagnostic =
        validate_value_occurrence_header_for_session_binding(&header, &[test_binding_format()])
            .expect_err("invalid header should be rejected");

    assert_eq!(
        diagnostic.code.as_deref(),
        Some("runtime.value-binding.invalid-header")
    );
}

#[test]
fn value_occurrence_header_guard_rejects_unknown_binding() {
    let mut header = test_occurrence_header();
    header.binding_id = "missing_edge".to_owned();

    let diagnostic =
        validate_value_occurrence_header_for_session_binding(&header, &[test_binding_format()])
            .expect_err("unknown binding should be rejected");

    assert_eq!(
        diagnostic.code.as_deref(),
        Some("runtime.value-binding.unknown-binding")
    );
}

#[test]
fn value_occurrence_header_guard_rejects_stale_binding_metadata() {
    let binding = test_binding_format();
    let mut stale_epoch = test_occurrence_header();
    stale_epoch.binding_epoch = 1;
    let epoch_diagnostic = validate_value_occurrence_header_for_session_binding(
        &stale_epoch,
        std::slice::from_ref(&binding),
    )
    .expect_err("stale epoch should be rejected");
    assert_eq!(
        epoch_diagnostic.code.as_deref(),
        Some("runtime.value-binding.stale-epoch")
    );

    let mut stale_format = test_occurrence_header();
    stale_format.format_revision = 6;
    let format_diagnostic =
        validate_value_occurrence_header_for_session_binding(&stale_format, &[binding])
            .expect_err("stale format revision should be rejected");
    assert_eq!(
        format_diagnostic.code.as_deref(),
        Some("runtime.value-binding.stale-format-revision")
    );
}

#[test]
fn idempotency_results_follow_retained_event_window() {
    let state = RuntimeRealtimeState::new("default", 2);
    let identity = state.issue_connection_identity(None);

    for sequence in 1..=3 {
        let cursor = state.cursor_for(sequence);
        let idempotency_key = format!("key-{sequence}");
        state.remember_ack(RememberAckInput {
            identity: &identity,
            message_type: "presence.update",
            idempotency_key: &idempotency_key,
            event_cursor: &cursor,
            event_sequence: sequence,
            ack_payload: json!({ "eventCursor": cursor }),
            emitted_results: Vec::new(),
        });
        state.publish(realtime_event("default", sequence, &cursor));
    }

    let idempotency_results = state
        .idempotency_results
        .lock()
        .expect("runtime realtime idempotency lock should not be poisoned");
    assert_eq!(idempotency_results.len(), 2);
    assert!(
        !idempotency_results.contains_key(&RuntimeRealtimeIdempotencyScope {
            client_id: identity.client_id.clone(),
            window_id: identity.window_id.clone(),
            message_type: "presence.update".to_owned(),
            idempotency_key: "key-1".to_owned(),
        })
    );
    assert!(
        idempotency_results.contains_key(&RuntimeRealtimeIdempotencyScope {
            client_id: identity.client_id.clone(),
            window_id: identity.window_id.clone(),
            message_type: "presence.update".to_owned(),
            idempotency_key: "key-2".to_owned(),
        })
    );
    assert!(
        idempotency_results.contains_key(&RuntimeRealtimeIdempotencyScope {
            client_id: identity.client_id.clone(),
            window_id: identity.window_id.clone(),
            message_type: "presence.update".to_owned(),
            idempotency_key: "key-3".to_owned(),
        })
    );
}

#[test]
fn replay_after_reports_cursor_diagnostics_and_marks_replayed_events() {
    let state = RuntimeRealtimeState::new("default", 2);
    let current_cursor = state.current_cursor();
    let (incarnation_id, _) = current_cursor
        .rsplit_once(':')
        .expect("runtime cursor should include sequence separator");

    let invalid_shape = state
        .replay_after("not-a-runtime-cursor")
        .expect_err("cursor without sequence separator should be rejected");
    assert_eq!(invalid_shape.code, "realtime.cursor.invalid");

    let wrong_incarnation = state
        .replay_after("other-incarnation:0")
        .expect_err("cursor from another incarnation should be rejected");
    assert_eq!(
        wrong_incarnation.code,
        "realtime.cursor.incarnation-mismatch"
    );

    let invalid_sequence = state
        .replay_after(&format!("{incarnation_id}:not-a-number"))
        .expect_err("cursor with non-numeric sequence should be rejected");
    assert_eq!(invalid_sequence.code, "realtime.cursor.invalid");

    let ahead = state
        .replay_after(&format!("{incarnation_id}:1"))
        .expect_err("cursor ahead of the current sequence should require sync");
    assert_eq!(ahead.code, "realtime.cursor.unknown");

    for expected_sequence in 1..=3 {
        let sequence = state.next_event_sequence();
        assert_eq!(sequence, expected_sequence);
        let cursor = state.cursor_for(sequence);
        state.publish(realtime_event("default", sequence, &cursor));
    }

    let expired = state
        .replay_after(&state.cursor_for(0))
        .expect_err("cursor before retained window should require sync");
    assert_eq!(expired.code, "realtime.cursor.expired");

    let replay = state
        .replay_after(&state.cursor_for(2))
        .expect("cursor inside retained window should replay later events");
    assert_eq!(replay.high_water_sequence, 3);
    assert_eq!(replay.events.len(), 1);
    assert_eq!(replay.events[0].sequence, Some(3));
    assert_eq!(replay.events[0].payload["replayed"], true);
}

#[test]
fn presence_entries_are_ttl_pruned_and_count_bounded() {
    let state = RuntimeRealtimeState::new("default", 1);
    let now = SystemTime::now();
    let expired_at = now
        .checked_sub(Duration::from_secs(1))
        .expect("test time should support subtraction");
    let future = now + Duration::from_secs(60);
    let expired = state.issue_connection_identity(None);
    let active_a = state.issue_connection_identity(None);
    let active_b = state.issue_connection_identity(None);
    let active_c = state.issue_connection_identity(None);

    state.remember_presence(&expired, json!({ "client": "expired" }), expired_at, 1);
    state.remember_presence(&active_a, json!({ "client": "a" }), future, 2);
    state.remember_presence(&active_b, json!({ "client": "b" }), future, 3);
    state.remember_presence(&active_c, json!({ "client": "c" }), future, 4);

    let presence = state
        .presence
        .lock()
        .expect("runtime realtime presence lock should not be poisoned");
    assert_eq!(presence.len(), 2);
    assert!(!presence.contains_key(&format!("{}:{}", expired.client_id, expired.window_id)));
    assert!(!presence.contains_key(&format!("{}:{}", active_a.client_id, active_a.window_id)));
    assert!(presence.contains_key(&format!("{}:{}", active_b.client_id, active_b.window_id)));
    assert!(presence.contains_key(&format!("{}:{}", active_c.client_id, active_c.window_id)));
}

#[test]
fn hello_catalog_and_error_envelopes_preserve_client_context() {
    let registry = crate::RuntimeSessionRegistry::dry_preview();
    let record = registry.default_record();
    let identity = record
        .realtime
        .issue_connection_identity(Some(RuntimeRealtimeResumeIdentity {
            client_id: "resumed-client".to_owned(),
            window_id: "resumed-window".to_owned(),
            expires_at: SystemTime::now() + Duration::from_secs(60),
        }));
    let mut frame = client_frame(
        &record.id,
        "session.hello",
        "hello-1",
        json!({ "nodeCatalog": { "mode": "always" } }),
    );
    frame.cursor = Some("cursor-from-frame".to_owned());

    let decoded = decode_hello_payload(&frame);
    assert_eq!(decoded.last_cursor.as_deref(), Some("cursor-from-frame"));
    assert_eq!(
        decoded.node_catalog.as_ref().map(|request| request.mode),
        Some(NodeCatalogHelloMode::Always)
    );

    let snapshot = current_snapshot(&record);
    let catalog_snapshot = node_catalog_snapshot_for_record(&record);
    let current_revision = serde_json::to_value(&catalog_snapshot.catalog_revision)
        .expect("catalog revision should serialize");
    let included_catalog = hello_node_catalog_payload(&record, decoded.node_catalog.as_ref());
    assert_eq!(included_catalog["status"], "included");
    assert!(included_catalog.get("snapshot").is_some());
    let unchanged_catalog = hello_node_catalog_payload(
        &record,
        Some(&NodeCatalogHelloRequest {
            mode: NodeCatalogHelloMode::IfChanged,
            known_revision: Some(current_revision.clone()),
        }),
    );
    assert_eq!(unchanged_catalog["status"], "unchanged");
    assert!(unchanged_catalog.get("snapshot").is_none());
    assert!(catalog_revision_matches(
        Some(&Value::String(
            catalog_snapshot.catalog_revision.value.clone()
        )),
        &catalog_snapshot.catalog_revision,
    ));
    assert!(catalog_revision_matches(
        Some(&current_revision),
        &catalog_snapshot.catalog_revision,
    ));
    assert!(!catalog_revision_matches(
        None,
        &catalog_snapshot.catalog_revision
    ));

    let attached = session_attached(
        &record,
        &identity,
        &frame,
        &snapshot,
        decoded.node_catalog.as_ref(),
    );
    assert_eq!(attached.message_type, "session.attached");
    assert_eq!(attached.correlation_id.as_deref(), Some("hello-1"));
    assert_eq!(attached.payload["clientId"], identity.client_id);
    assert_eq!(attached.payload["nodeCatalog"]["status"], "included");
    assert_eq!(
        attached.payload["currentRevisions"]["sessionRevision"],
        snapshot.session_revision
    );

    let sync = session_sync_required(
        &record,
        &identity,
        &frame,
        &snapshot,
        sync_required_diagnostic(
            "realtime.cursor.test",
            "test sync required",
            Some(json!({ "currentCursor": record.realtime.current_cursor() })),
        ),
        Some(&NodeCatalogHelloRequest {
            mode: NodeCatalogHelloMode::IfChanged,
            known_revision: Some(current_revision),
        }),
    );
    assert_eq!(sync.message_type, "session.syncRequired");
    assert_eq!(sync.payload["diagnostic"]["code"], "realtime.cursor.test");
    assert_eq!(sync.payload["nodeCatalog"]["status"], "unchanged");

    let error = runtime_error(
        &record.id,
        Some(&identity),
        Some(&frame),
        "realtime.frame.test",
        "test error",
        Some(json!({ "field": "payload" })),
    );
    assert_eq!(error.message_type, "runtime.error");
    assert_eq!(
        error.connection_id.as_deref(),
        Some(identity.connection_id.as_str())
    );
    assert_eq!(error.command_id.as_deref(), Some("hello-1-command"));
    assert_eq!(error.correlation_id.as_deref(), Some("hello-1"));
    assert_eq!(error.payload["diagnostic"]["details"]["field"], "payload");

    let internal = empty_correlation_frame(&record.id);
    assert_eq!(internal.message_type, "runtime.internal");
    assert_eq!(internal.session_id, record.id);
    assert_eq!(internal.payload, Value::Null);
}

#[test]
fn command_handlers_reject_invalid_payloads_and_clamp_presence_ttl() {
    let registry = crate::RuntimeSessionRegistry::dry_preview();
    let record = registry.default_record();
    let identity = record.realtime.issue_connection_identity(None);

    let missing_presence_key = handle_presence_update(
        &record,
        &identity,
        client_frame(
            &record.id,
            "presence.update",
            "presence-missing-key",
            json!({ "presence": { "tool": "select" } }),
        ),
    )
    .expect_err("presence updates require idempotency keys");
    assert_eq!(
        missing_presence_key.code,
        "realtime.command.idempotency-key-required"
    );

    let mut invalid_presence = client_frame(
        &record.id,
        "presence.update",
        "presence-invalid",
        Value::String("bad-presence".to_owned()),
    );
    invalid_presence.idempotency_key = Some("presence-invalid-key".to_owned());
    let invalid_presence = handle_presence_update(&record, &identity, invalid_presence)
        .expect_err("non-object presence payload should be rejected");
    assert_eq!(invalid_presence.code, "realtime.presence.invalid-payload");

    let mut valid_presence = client_frame(
        &record.id,
        "presence.update",
        "presence-valid",
        json!({ "presence": { "tool": "draw" }, "ttlMs": 1 }),
    );
    valid_presence.idempotency_key = Some("presence-valid-key".to_owned());
    let (ack, event) = handle_presence_update(&record, &identity, valid_presence)
        .expect("valid presence update should ack and publish");
    let event = event.expect("presence update should create a realtime event");
    assert_eq!(ack.message_type, "command.ack");
    assert_eq!(ack.payload["accepted"], true);
    assert_eq!(event.message_type, "presence.updated");
    assert_eq!(event.payload["presence"]["presence"]["tool"], "draw");
    assert_eq!(event.payload["presence"]["ttlMs"], 1000);

    let graph_missing_key = handle_graph_command(
        &record,
        &identity,
        client_frame(
            &record.id,
            "graph.command",
            "graph-missing-key",
            json!({ "kind": "node.resolve" }),
        ),
    )
    .expect_err("graph commands require idempotency keys");
    assert_eq!(
        graph_missing_key.code,
        "realtime.command.idempotency-key-required"
    );

    let mut invalid_graph = client_frame(
        &record.id,
        "graph.command",
        "graph-invalid",
        json!({ "baseSessionRevision": 0 }),
    );
    invalid_graph.idempotency_key = Some("graph-invalid-key".to_owned());
    let invalid_graph = handle_graph_command(&record, &identity, invalid_graph)
        .expect_err("missing graph kind should be rejected by payload decoding");
    assert_eq!(invalid_graph.code, "realtime.graph.invalid-payload");
}

#[test]
fn node_command_result_serializes_resolution_ports_diagnostics_and_input() {
    let payload = graph_payload(json!({
        "kind": "node.replace",
        "target": root_target("1"),
        "objectText": "sig~ 440",
        "nodeId": "oscillator",
        "requestedNodeId": "requested-oscillator",
        "unresolvedPolicy": "reject",
        "interfaceIncidentEdgePolicy": "reject",
        "surfacePath": ["root", "oscillator"]
    }));
    let resolution = object_text_resolution_with_all_port_variants();

    let node_result = node_command_result(
        &payload,
        Some(&resolution),
        Some("oscillator"),
        vec!["stale-edge".to_owned()],
        Some(json!({ "message": ControlMessage::bang() })),
    );

    assert_eq!(node_result["kind"], "node.replace");
    assert_eq!(node_result["nodeId"], "oscillator");
    assert_eq!(node_result["requestedNodeId"], "requested-oscillator");
    assert_eq!(node_result["interfaceIncidentEdgePolicy"], "reject");
    assert_eq!(node_result["droppedEdgeIds"], json!(["stale-edge"]));
    let rates = node_result["resolution"]["ports"]
        .as_array()
        .expect("ports should serialize")
        .iter()
        .map(|port| port["rate"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        rates,
        vec![
            "event", "control", "audio", "render", "gpu", "resource", "io"
        ]
    );
    let activations = node_result["resolution"]["ports"]
        .as_array()
        .expect("ports should serialize")
        .iter()
        .map(|port| port["activation"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        activations[..3],
        [
            Value::String("trigger".to_owned()),
            Value::String("latched".to_owned()),
            Value::String("passive".to_owned())
        ]
    );
    assert_eq!(
        node_result["resolution"]["diagnostics"][0]["code"],
        "object-text.test-warning"
    );

    let diagnostics = object_text_runtime_diagnostics(&resolution);
    assert_eq!(
        diagnostics[0].code.as_deref(),
        Some("object-text.test-warning")
    );
    assert_eq!(
        diagnostics[0]
            .details
            .as_ref()
            .expect("object text diagnostics should include structured details")["candidateCount"],
        1
    );

    let input_result = node_input_result(
        &RuntimeControlEventRequest {
            node_id: "oscillator".to_owned(),
            port_id: "frequency".to_owned(),
            message: ControlMessage::from_value(ControlValue::float(440.0)),
        },
        &RuntimeControlEventResponse {
            ok: true,
            changed: true,
            control_revision: Some(7),
            emitted: Vec::new(),
            diagnostics: Vec::new(),
        },
    );
    assert_eq!(input_result["accepted"], true);
    assert_eq!(input_result["controlRevision"], 7);
}

#[test]
fn object_command_materialization_respects_params_and_unresolved_policy() {
    let session = crate::RuntimeSession::default();
    let payload = graph_payload(json!({
        "kind": "node.create",
        "target": root_target("1"),
        "objectText": "float 1",
        "params": { "frequency": 880.0 }
    }));
    let materialized = materialize_object_command_node(
        &session,
        &payload,
        &resolved_float_object_text("float 1"),
        "float_1",
    )
    .expect("resolved object text should materialize");
    assert_eq!(materialized.0.params["frequency"], 880.0);

    let unresolved = ObjectRegistry::first_party_core().resolve("missingObject 1");
    let diagnostic_payload = graph_payload(json!({
        "kind": "node.create",
        "target": root_target("1"),
        "objectText": "missingObject 1",
        "params": { "label": "keep me" }
    }));
    let diagnostic_node =
        materialize_object_command_node(&session, &diagnostic_payload, &unresolved, "missing_1")
            .expect("default unresolved policy should materialize a diagnostic node");
    assert_eq!(diagnostic_node.0.id, "missing_1");
    assert_eq!(diagnostic_node.0.params["label"], "keep me");

    let reject_payload = graph_payload(json!({
        "kind": "node.create",
        "target": root_target("1"),
        "objectText": "missingObject 1",
        "unresolvedPolicy": "reject"
    }));
    assert!(
        materialize_object_command_node(&session, &reject_payload, &unresolved, "missing_2")
            .is_none()
    );
}

#[test]
fn object_command_helpers_validate_required_fields_and_targets() {
    let identity = test_identity();
    let frame = client_frame("default", "graph.command", "graph-helpers", Value::Null);
    let mut session = crate::RuntimeSession::default();

    let missing_resolve = apply_object_resolve_graph_command(
        &session,
        &graph_payload(json!({ "kind": "node.resolve" })),
    );
    assert_response_diagnostic_code(
        &missing_resolve.response,
        "graph.command.object-text-required",
    );
    let missing_create = apply_object_create_graph_command(
        &mut session,
        &identity,
        &frame,
        &graph_payload(json!({ "kind": "node.create" })),
    );
    assert_response_diagnostic_code(
        &missing_create.response,
        "graph.command.object-text-required",
    );
    let missing_replace = apply_object_replace_graph_command(
        &mut session,
        &identity,
        &frame,
        &graph_payload(json!({ "kind": "node.replace" })),
    );
    assert_response_diagnostic_code(
        &missing_replace.response,
        "graph.command.object-text-required",
    );
    let missing_delete_node = apply_node_delete_graph_command(
        &mut session,
        &identity,
        &frame,
        &graph_payload(json!({ "kind": "node.delete", "target": root_target("1") })),
    );
    assert_response_diagnostic_code(
        &missing_delete_node.response,
        "graph.command.node-id-required",
    );
    let missing_update_node = apply_node_update_graph_command(
        &mut session,
        &identity,
        &frame,
        &graph_payload(json!({ "kind": "node.update", "target": root_target("1") })),
    );
    assert_response_diagnostic_code(
        &missing_update_node.response,
        "graph.command.node-id-required",
    );
    let empty_update = apply_node_update_graph_command(
        &mut session,
        &identity,
        &frame,
        &graph_payload(json!({
            "kind": "node.update",
            "target": root_target("1"),
            "nodeId": "value_1"
        })),
    );
    assert_response_diagnostic_code(&empty_update.response, "graph.command.params-required");

    let no_target = validate_object_command_target(
        &session,
        &graph_payload(json!({ "kind": "node.resolve" })),
        true,
    )
    .expect_err("node commands require a target");
    assert_response_diagnostic_code(&no_target, "graph.command.target-required");
    let revision_conflict = validate_object_command_target(
        &session,
        &graph_payload(json!({
            "kind": "node.create",
            "target": root_target("1"),
            "baseGraphRevision": "2"
        })),
        false,
    )
    .expect_err("baseGraphRevision must agree with target.baseRevision");
    assert!(revision_conflict.conflict);
    assert_response_diagnostic_code(&revision_conflict, "graph.command.target-revision-conflict");
    let missing_graph = validate_object_command_target(
        &session,
        &graph_payload(json!({ "kind": "node.resolve", "target": root_target("1") })),
        true,
    )
    .expect_err("node.resolve requires an existing target graph");
    assert_response_diagnostic_code(&missing_graph, "node.target.missing-graph");
}

#[test]
fn node_id_generation_helpers_are_stable_for_object_text_commands() {
    assert_eq!(
        node_id_slug("123 Weird Object!!"),
        Some("node_123_weird_object".to_owned())
    );
    assert_eq!(node_id_slug("   "), None);
    assert_eq!(
        next_generated_node_id(
            "osc",
            &["osc".to_owned(), "osc_2".to_owned(), "other".to_owned()]
        ),
        "osc_3"
    );
}

#[test]
fn graph_command_validation_covers_view_and_change_set_rejections() {
    let registry = crate::RuntimeSessionRegistry::dry_preview();
    let record = registry.default_record();
    let identity = test_identity();
    let frame = client_frame(&record.id, "graph.command", "graph-validate", Value::Null);

    let session_conflict = apply_graph_command(
        &record,
        &identity,
        &frame,
        &graph_payload(json!({ "kind": "node.resolve", "baseSessionRevision": 99 })),
    );
    assert!(session_conflict.response.conflict);
    assert_response_diagnostic_code(
        &session_conflict.response,
        "graph.command.session-revision-conflict",
    );

    let missing_view_patch = apply_graph_command(
        &record,
        &identity,
        &frame,
        &graph_payload(json!({ "kind": "view.patch" })),
    );
    assert_response_diagnostic_code(
        &missing_view_patch.response,
        "graph.command.view-patch-required",
    );

    let view_revision_conflict = apply_graph_command(
        &record,
        &identity,
        &frame,
        &graph_payload(json!({
            "kind": "view.patch",
            "baseViewRevision": 2,
            "viewPatch": { "baseViewRevision": 1, "ops": [] }
        })),
    );
    assert!(view_revision_conflict.response.conflict);
    assert_response_diagnostic_code(
        &view_revision_conflict.response,
        "graph.command.view-revision-conflict",
    );

    let graph_revision_conflict = apply_graph_command(
        &record,
        &identity,
        &frame,
        &graph_payload(json!({
            "kind": "view.patch",
            "baseGraphRevision": "1",
            "viewPatch": { "baseViewRevision": 0, "ops": [] }
        })),
    );
    assert!(graph_revision_conflict.response.conflict);
    assert_response_diagnostic_code(
        &graph_revision_conflict.response,
        "graph.command.graph-revision-conflict",
    );

    let unsupported_view_target = apply_graph_command(
        &record,
        &identity,
        &frame,
        &graph_payload(json!({
            "kind": "view.patch",
            "target": package_patch_target("1"),
            "viewPatch": { "baseViewRevision": 0, "ops": [] }
        })),
    );
    assert_response_diagnostic_code(
        &unsupported_view_target.response,
        "graph.command.view-target-unsupported",
    );

    let target_revision_conflict = apply_graph_command(
        &record,
        &identity,
        &frame,
        &graph_payload(json!({
            "kind": "view.patch",
            "target": root_target("1"),
            "viewPatch": { "baseViewRevision": 0, "ops": [] }
        })),
    );
    assert!(target_revision_conflict.response.conflict);
    assert_response_diagnostic_code(
        &target_revision_conflict.response,
        "graph.command.target-revision-conflict",
    );

    let missing_change_target = apply_graph_command(
        &record,
        &identity,
        &frame,
        &graph_payload(json!({ "kind": "graph.changeSet" })),
    );
    assert_response_diagnostic_code(
        &missing_change_target.response,
        "graph.command.target-required",
    );

    let empty_changes = apply_graph_command(
        &record,
        &identity,
        &frame,
        &graph_payload(json!({
            "kind": "graph.changeSet",
            "target": root_target("1"),
            "changes": []
        })),
    );
    assert_response_diagnostic_code(&empty_changes.response, "graph.command.changes-required");

    let change_revision_conflict = apply_graph_command(
        &record,
        &identity,
        &frame,
        &graph_payload(json!({
            "kind": "graph.changeSet",
            "target": root_target("1"),
            "baseGraphRevision": "2",
            "changes": [
                { "op": "node.delete", "changeId": "delete-value", "nodeId": "value_1" }
            ]
        })),
    );
    assert!(change_revision_conflict.response.conflict);
    assert_response_diagnostic_code(
        &change_revision_conflict.response,
        "graph.command.target-revision-conflict",
    );
}

#[test]
fn cached_ack_and_control_event_helpers_preserve_payload_flags() {
    let registry = crate::RuntimeSessionRegistry::dry_preview();
    let record = registry.default_record();
    let identity = test_identity();
    let mut frame = client_frame(&record.id, "graph.command", "cached-ack", Value::Null);
    frame.idempotency_key = Some("idem-cached".to_owned());

    let cached = RuntimeRealtimeCachedCommandResult {
        event_cursor: "cursor-cached".to_owned(),
        ack_payload: json!({ "accepted": true, "cached": false }),
        emitted_results: Vec::new(),
    };
    let graph_ack = graph_ack_from_cached(&record, &identity, &frame, cached.clone());
    assert_eq!(graph_ack.message_type, "graph.ack");
    assert_eq!(graph_ack.payload["cached"], true);
    assert_eq!(graph_ack.payload["eventCursor"], "cursor-cached");

    let command_ack = command_ack_from_cached(&record, &identity, &frame, cached);
    assert_eq!(command_ack.message_type, "command.ack");
    assert_eq!(command_ack.payload["cached"], true);

    let request = RuntimeControlEventRequest {
        node_id: "value_1".to_owned(),
        port_id: "value".to_owned(),
        message: ControlMessage::bang(),
    };
    let mut response = RuntimeControlEventResponse {
        ok: true,
        changed: false,
        control_revision: Some(1),
        emitted: Vec::new(),
        diagnostics: Vec::new(),
    };
    assert!(
        control_emitted_event(
            &record,
            &identity,
            &frame,
            &request,
            &mut response,
            BTreeMap::new(),
            RealtimeEventPosition {
                sequence: 3,
                cursor: "cursor-3",
            },
        )
        .is_none()
    );

    let mut changed_values = BTreeMap::new();
    changed_values.insert("value_1".to_owned(), ControlValue::float(0.5));
    let event = control_emitted_event(
        &record,
        &identity,
        &frame,
        &request,
        &mut response,
        changed_values,
        RealtimeEventPosition {
            sequence: 4,
            cursor: "cursor-4",
        },
    )
    .expect("changed control values should produce an event");
    assert_eq!(event.message_type, "control.emitted");
    assert_eq!(event.sequence, Some(4));
    assert_eq!(event.payload["values"]["value_1"]["value"], 0.5);
}
