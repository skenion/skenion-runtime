use super::*;

fn assert_kind(resolution: &ObjectSpecResolution, kind: &str) {
    assert!(resolution.ok(), "{resolution:?}");
    assert_eq!(resolution.resolved_kind.as_deref(), Some(kind));
    assert_eq!(resolution.resolved_kind_version.as_deref(), Some("0.1.0"));
}

fn assert_diagnostic(resolution: &ObjectSpecResolution, code: &str) {
    assert_eq!(resolution.resolved_kind, None);
    assert_eq!(resolution.diagnostics[0].code, code);
}

fn patch_definition(id: &str) -> PatchDefinitionCurrent {
    serde_json::from_value(json!({
        "id": id,
        "revision": "7",
        "metadata": {
            "title": format!("Patch {id}"),
            "description": format!("{id} reusable patch")
        },
        "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": format!("{id}-graph"),
            "revision": "7",
            "nodes": [
                {
                    "id": "patch_in",
                    "kind": "object.core.inlet",
                    "kindVersion": "0.1.0",
                    "params": { "portId": "value", "label": "Value" },
                    "ports": [
                        {
                            "id": "out",
                            "direction": "output",
                            "type": "value.core.float32",
                            "rate": "control"
                        }
                    ]
                },
                {
                    "id": "patch_out",
                    "kind": "object.core.outlet",
                    "kindVersion": "0.1.0",
                    "params": { "portId": "result", "label": "Result" },
                    "ports": [
                        {
                            "id": "in",
                            "direction": "input",
                            "type": "value.core.float32",
                            "rate": "control"
                        }
                    ]
                }
            ],
            "edges": []
        }
    }))
    .expect("patch definition should deserialize")
}

#[test]
fn resolves_runtime_control_aliases_and_validates_args() {
    let add = resolve_object_spec_v01("[+ 1e3]");
    assert!(add.ok());
    assert_eq!(add.display_text, "+ 1e3");
    assert_eq!(add.class_symbol, "+");
    assert_eq!(
        add.resolved_kind.as_deref(),
        Some("object.core.operator.add")
    );
    assert_eq!(add.resolved_kind_version.as_deref(), Some("0.1.0"));
    assert_eq!(add.params["right"], json!(1000.0));
    assert_eq!(add.instance_ports[0].id, "in");

    let sqrt = resolve_object_spec_v01("sqrt 2");
    assert_eq!(sqrt.diagnostics[0].code, "object-spec.invalid-arg-count");

    let invalid = resolve_object_spec_v01("+ true");
    assert_eq!(invalid.diagnostics[0].code, "object-spec.invalid-arg-type");

    for (input, kind, param, value) in [
        ("- -2", "object.core.operator.sub", "right", json!(-2.0)),
        ("/ 4", "object.core.operator.div", "right", json!(4.0)),
        ("* 3", "object.core.operator.mul", "right", json!(3.0)),
        ("pow 2", "object.core.operator.pow", "right", json!(2.0)),
        ("max 8", "object.core.operator.max", "right", json!(8.0)),
        ("min 1", "object.core.operator.min", "right", json!(1.0)),
    ] {
        let resolution = resolve_object_spec_v01(input);
        assert_kind(&resolution, kind);
        assert_eq!(resolution.params[param], value);
        assert_eq!(resolution.instance_ports.len(), 3);
    }

    let sqrt = resolve_object_spec_v01("sqrt");
    assert_kind(&sqrt, "object.core.operator.sqrt");
    assert_eq!(sqrt.instance_ports.len(), 2);

    let default_add = resolve_object_spec_v01("object.core.operator.add");
    assert_kind(&default_add, "object.core.operator.add");
    assert_eq!(default_add.params["right"], json!(0.0));

    assert_diagnostic(
        &resolve_object_spec_v01("sqrt 1"),
        "object-spec.invalid-arg-count",
    );
    assert_diagnostic(
        &resolve_object_spec_v01("+ 1 2"),
        "object-spec.invalid-arg-count",
    );
    assert_diagnostic(
        &resolve_object_spec_v01("object.core.operator.mul false"),
        "object-spec.invalid-arg-type",
    );
}

#[test]
fn resolves_runtime_value_audio_and_subpatch_aliases() {
    let float = resolve_object_spec_v01("f 0.25");
    assert!(float.ok());
    assert_eq!(float.resolved_kind.as_deref(), Some("object.core.float"));
    assert_eq!(float.params["value"], json!(0.25));

    let osc = resolve_object_spec_v01("osc~ 220");
    assert!(osc.ok());
    assert_eq!(osc.resolved_kind.as_deref(), Some("object.core.audio.osc"));
    assert_eq!(osc.params["frequency"], json!(220.0));

    let mul = resolve_object_spec_v01("*~");
    assert!(mul.ok());
    assert_eq!(
        mul.resolved_kind.as_deref(),
        Some("object.core.audio.operator.mul")
    );
    assert_eq!(mul.instance_ports.len(), 3);

    let scalar_mul = resolve_object_spec_v01("*~ 0.5");
    assert_eq!(
        scalar_mul.diagnostics[0].code,
        "object-spec.invalid-arg-count"
    );

    let unsupported = resolve_object_spec_v01("+~");
    assert_eq!(
        unsupported.diagnostics[0].code,
        "object-spec.unsupported-first-party"
    );

    for input in [
        "-~",
        "/~",
        "sqrt~",
        "phasor~",
        "object.core.audio.operator.add",
        "object.core.audio.operator.sqrt",
        "object.core.audio.phasor",
    ] {
        assert_diagnostic(
            &resolve_object_spec_v01(input),
            "object-spec.unsupported-first-party",
        );
    }

    let sig = resolve_object_spec_v01("sig~");
    assert_kind(&sig, "object.core.audio.sig");
    assert_eq!(sig.params["value"], json!(0.0));

    let invalid_sig = resolve_object_spec_v01("sig~ false");
    assert_diagnostic(&invalid_sig, "object-spec.invalid-arg-type");
    assert_diagnostic(
        &resolve_object_spec_v01("sig~ 1 2"),
        "object-spec.invalid-arg-count",
    );

    let osc = resolve_object_spec_v01("object.core.audio.osc 220");
    assert_kind(&osc, "object.core.audio.osc");
    assert_eq!(osc.params["frequency"], json!(220.0));
    assert_diagnostic(
        &resolve_object_spec_v01("osc~ nope"),
        "object-spec.invalid-arg-type",
    );

    let audio_input = resolve_object_spec_v01("adc~");
    assert_kind(&audio_input, "object.core.audio.input");
    assert_eq!(audio_input.instance_ports[0].id, "left");

    let audio_output = resolve_object_spec_v01("dac~");
    assert_kind(&audio_output, "object.core.audio.output");
    assert_eq!(audio_output.instance_ports[0].id, "left");

    let invalid_audio_output = resolve_object_spec_v01("dac~ 1");
    assert_diagnostic(&invalid_audio_output, "object-spec.invalid-arg-count");

    let subpatch = resolve_object_spec_v01("p voice");
    assert!(subpatch.ok());
    assert_eq!(
        subpatch.resolved_kind.as_deref(),
        Some("object.core.subpatch")
    );
    assert_eq!(subpatch.params["patchRef"], json!("voice"));
}

#[test]
fn resolves_runtime_value_boxes_and_boundary_aliases() {
    for (input, kind, value) in [
        ("float", "object.core.float", json!(0)),
        ("int -7", "object.core.int", json!(-7)),
        ("uint 9", "object.core.uint", json!(9)),
    ] {
        let resolution = resolve_object_spec_v01(input);
        assert_kind(&resolution, kind);
        assert_eq!(resolution.params["value"], value);
        assert_eq!(resolution.instance_ports.len(), 3);
    }

    assert_diagnostic(
        &resolve_object_spec_v01("int 1.5"),
        "object-spec.invalid-arg-type",
    );
    assert_diagnostic(
        &resolve_object_spec_v01("uint -1"),
        "object-spec.invalid-arg-type",
    );
    assert_diagnostic(
        &resolve_object_spec_v01("float 1 2"),
        "object-spec.invalid-arg-count",
    );

    let bang = resolve_object_spec_v01("bang");
    assert_kind(&bang, "object.core.bang");
    assert!(bang.params.is_empty());
    assert_eq!(bang.instance_ports[1].port_type, "value.core.bang");
    assert_diagnostic(
        &resolve_object_spec_v01("object.core.bang 1"),
        "object-spec.invalid-arg-count",
    );

    let float_alias = resolve_object_spec_v01("f 1.5");
    assert_kind(&float_alias, "object.core.float");
    assert_eq!(float_alias.params["value"], json!(1.5));
    assert_diagnostic(
        &resolve_object_spec_v01("float true"),
        "object-spec.invalid-arg-type",
    );

    let message = resolve_object_spec_v01("message set gain");
    assert_kind(&message, "object.core.message");
    assert_eq!(message.params["text"], json!("set gain"));
    let empty_message = resolve_object_spec_v01("msg");
    assert_kind(&empty_message, "object.core.message");
    assert_eq!(empty_message.params["text"], json!(""));

    let comment = resolve_object_spec_v01("comment hello world");
    assert_kind(&comment, "object.core.comment");
    assert_eq!(comment.params["text"], json!("hello world"));
    assert_eq!(comment.instance_ports.len(), 1);
    let empty_comment = resolve_object_spec_v01("object.core.comment");
    assert_kind(&empty_comment, "object.core.comment");
    assert_eq!(empty_comment.params["text"], json!(""));

    let inlet = resolve_object_spec_v01("inlet left");
    assert_kind(&inlet, "object.core.inlet");
    assert_eq!(inlet.params["portId"], json!("left"));

    let anonymous_outlet = resolve_object_spec_v01("outlet");
    assert_kind(&anonymous_outlet, "object.core.outlet");
    assert!(anonymous_outlet.params.is_empty());
    let named_outlet = resolve_object_spec_v01("object.core.outlet right");
    assert_kind(&named_outlet, "object.core.outlet");
    assert_eq!(named_outlet.params["portId"], json!("right"));

    assert_diagnostic(
        &resolve_object_spec_v01("p"),
        "object-spec.invalid-arg-count",
    );
    assert_diagnostic(
        &resolve_object_spec_v01("p true"),
        "object-spec.invalid-arg-type",
    );
    assert_diagnostic(
        &resolve_object_spec_v01("inlet left right"),
        "object-spec.invalid-arg-count",
    );
    assert_diagnostic(
        &resolve_object_spec_v01("outlet 1"),
        "object-spec.invalid-arg-type",
    );
}

#[test]
fn rejects_payload_identities_as_object_spec() {
    for input in [
        "value",
        "data",
        "payload",
        "value.core.float32",
        "bool",
        "string",
        "object.core.bool",
        "object.core.string",
        "value.core.bang",
        "value.core.message",
        "value.core.string",
        "value.core.tensor",
        "data.vendor.payload",
        "payload.vendor.frame",
        "control.float",
    ] {
        let resolution = resolve_object_spec_v01(input);
        assert_eq!(resolution.resolved_kind, None);
        assert_eq!(
            resolution.diagnostics[0].code,
            "object-spec.payload-identity"
        );
    }
}

#[test]
fn reports_unresolved_and_syntax_diagnostics_without_runtime_mapping() {
    let unresolved = resolve_object_spec_v01("user.manipulator 1");
    assert_eq!(unresolved.diagnostics[0].code, "object-spec.unresolved");

    let invalid = resolve_object_spec_v01("[+ 1");
    assert_eq!(invalid.diagnostics[0].code, "object-spec.invalid-syntax");

    let empty = resolve_object_spec_v01("   ");
    assert_eq!(empty.diagnostics[0].code, "object-spec.empty");
}

#[test]
fn project_patch_registry_projects_catalog_and_resolution_edges() {
    let patch = patch_definition("my-patcher");
    let registry = ObjectRegistry::for_patch_library(std::slice::from_ref(&patch));
    let snapshot = registry.catalog_projection();
    let project_entry = snapshot
        .entries
        .iter()
        .find(|entry| {
            matches!(
                &entry.source,
                NodeCatalogSourceV01::ProjectPatch { patch_id, .. }
                    if patch_id == "my-patcher"
            )
        })
        .expect("project patch should appear in catalog");

    assert_eq!(project_entry.catalog_id, "project.my-patcher");
    assert_eq!(project_entry.canonical_object_spec, "my-patcher");
    assert_eq!(project_entry.display.title, "Patch my-patcher");
    assert_eq!(
        project_patch_object_kind("my patch/1"),
        "object.project.patch.my-patch-1"
    );
    assert_eq!(
        project_entry.definition.execution.model,
        skenion_contracts::ExecutionModelV01::Control
    );
    assert_eq!(project_entry.definition.ports.len(), 2);

    let direct = registry.resolve("my-patcher");
    assert_kind(&direct, "object.project.patch.my-patcher");
    assert_eq!(direct.params["patchRef"], json!("my-patcher"));
    assert_eq!(direct.params["patchRevision"], json!("7"));
    assert_eq!(direct.instance_ports.len(), 2);

    let explicit = registry.resolve("p my-patcher");
    assert_kind(&explicit, "object.project.patch.my-patcher");

    assert_diagnostic(
        &registry.resolve("my-patcher 1"),
        "object-spec.invalid-arg-count",
    );
    assert_diagnostic(&registry.resolve("p"), "object-spec.invalid-arg-count");
    assert_diagnostic(&registry.resolve("p true"), "object-spec.invalid-arg-type");

    let mismatched = construct_project_patch(
        ParsedObjectSpec {
            input: "p other".to_owned(),
            display_text: "p other".to_owned(),
            class_symbol: "p".to_owned(),
            creation_args: vec![ObjectSpecAtom::Symbol("other".to_owned())],
        },
        &ObjectRegistryCandidate {
            id: "project-patch:my-patcher".to_owned(),
            source: ObjectRegistrySource::ProjectPatch,
            aliases: vec!["my-patcher".to_owned()],
            kind: project_patch_object_kind("my-patcher"),
            kind_version: CURRENT_KIND_VERSION.to_owned(),
            display_name: "Patch my-patcher".to_owned(),
            constructor: None,
            catalog_category: None,
            project_patch: Some(ProjectPatchCandidate {
                patch_id: "my-patcher".to_owned(),
                revision: "7".to_owned(),
                description: Some("my-patcher reusable patch".to_owned()),
                interface_digest: skenion_contracts::compute_patch_interface_digest_v01(&patch),
                ports: project_patch_ports(&patch),
            }),
        },
    );
    assert_diagnostic(&mismatched, "object-spec.unresolved");
}

#[test]
fn reserved_providers_and_unconstructable_candidates_fail_closed() {
    let provider_registry = ObjectRegistry {
        candidates: vec![ObjectRegistryCandidate {
            id: "package:vendor.node".to_owned(),
            source: ObjectRegistrySource::PackageProvider,
            aliases: vec!["vendor.node".to_owned()],
            kind: "object.vendor.node".to_owned(),
            kind_version: CURRENT_KIND_VERSION.to_owned(),
            display_name: "Vendor Node".to_owned(),
            constructor: None,
            catalog_category: None,
            project_patch: None,
        }],
        allow_unchecked_project_patch_refs: false,
    };
    assert_diagnostic(
        &provider_registry.resolve("vendor.node"),
        "object-spec.provider-unavailable",
    );

    let ambiguous_registry = ObjectRegistry {
        candidates: vec![
            ObjectRegistryCandidate {
                id: "package:shared.node".to_owned(),
                source: ObjectRegistrySource::PackageProvider,
                aliases: vec!["shared.node".to_owned()],
                kind: "object.vendor.shared".to_owned(),
                kind_version: CURRENT_KIND_VERSION.to_owned(),
                display_name: "Package Shared".to_owned(),
                constructor: None,
                catalog_category: None,
                project_patch: None,
            },
            ObjectRegistryCandidate {
                id: "native:shared.node".to_owned(),
                source: ObjectRegistrySource::NativeProvider,
                aliases: vec!["shared.node".to_owned()],
                kind: "object.native.shared".to_owned(),
                kind_version: CURRENT_KIND_VERSION.to_owned(),
                display_name: "Native Shared".to_owned(),
                constructor: None,
                catalog_category: None,
                project_patch: None,
            },
        ],
        allow_unchecked_project_patch_refs: false,
    };
    let ambiguous = ambiguous_registry.resolve("shared.node");
    assert_diagnostic(&ambiguous, "object-spec.ambiguous");
    assert_eq!(ambiguous.candidates[0].source, "package-provider");
    assert_eq!(ambiguous.candidates[1].source, "native-provider");

    let missing_patch_metadata = ObjectRegistry {
        candidates: vec![ObjectRegistryCandidate {
            id: "project-patch:broken".to_owned(),
            source: ObjectRegistrySource::ProjectPatch,
            aliases: vec!["broken".to_owned()],
            kind: project_patch_object_kind("broken"),
            kind_version: CURRENT_KIND_VERSION.to_owned(),
            display_name: "Broken".to_owned(),
            constructor: None,
            catalog_category: None,
            project_patch: None,
        }],
        allow_unchecked_project_patch_refs: false,
    };
    assert_diagnostic(
        &missing_patch_metadata.resolve("broken"),
        "object-spec.unresolved",
    );

    let core_without_constructor = ObjectRegistry {
        candidates: vec![ObjectRegistryCandidate {
            id: "object.core.future".to_owned(),
            source: ObjectRegistrySource::FirstPartyCore,
            aliases: vec!["future".to_owned()],
            kind: "object.core.future".to_owned(),
            kind_version: CURRENT_KIND_VERSION.to_owned(),
            display_name: "Future".to_owned(),
            constructor: None,
            catalog_category: Some("Core"),
            project_patch: None,
        }],
        allow_unchecked_project_patch_refs: false,
    };
    assert_diagnostic(
        &core_without_constructor.resolve("future"),
        "object-spec.unresolved",
    );
}

#[test]
fn object_spec_materialization_and_port_projection_cover_diagnostic_edges() {
    let unresolved = ObjectSpecResolution {
        input: "future".to_owned(),
        display_text: "future".to_owned(),
        class_symbol: "future".to_owned(),
        creation_args: vec![
            ObjectSpecAtom::Float(1.5),
            ObjectSpecAtom::Int(2),
            ObjectSpecAtom::Bool(true),
            ObjectSpecAtom::Symbol("arg".to_owned()),
        ],
        resolved_kind: None,
        resolved_kind_version: None,
        params: Map::new(),
        instance_ports: Vec::new(),
        candidates: vec![ObjectSpecCandidateSummary {
            id: "package:future".to_owned(),
            source: "package-provider".to_owned(),
            kind: "object.future".to_owned(),
            display_name: "Future".to_owned(),
        }],
        diagnostics: Vec::new(),
    };

    let materialize_error = materialize_object_spec_node_v01(&unresolved, "future_1")
        .expect_err("unresolved object should not materialize as resolved node");
    assert_eq!(materialize_error.code, "object-spec.unresolved");
    assert!(materialize_error.message.contains("future"));

    let diagnostic_node = materialize_unresolved_object_spec_node_v01(&unresolved, "future_1");
    assert_eq!(diagnostic_node.kind, "object.core.unresolved");
    assert_eq!(diagnostic_node.params["candidateCount"], json!(1));
    assert_eq!(
        diagnostic_node.params["candidates"][0]["source"],
        "package-provider"
    );

    for (rate, expected_rate) in [
        (ObjectSpecPortRate::Event, PortRateCurrent::Event),
        (ObjectSpecPortRate::Render, PortRateCurrent::Render),
        (ObjectSpecPortRate::Gpu, PortRateCurrent::Gpu),
        (ObjectSpecPortRate::Resource, PortRateCurrent::Resource),
        (ObjectSpecPortRate::Io, PortRateCurrent::Io),
    ] {
        let current = object_spec_port_to_current(&input_port(
            "in",
            "value.core.message",
            rate,
            ObjectSpecPortActivation::Passive,
        ));
        assert_eq!(current.rate, Some(expected_rate));
        assert_eq!(
            current.trigger_mode,
            Some(skenion_contracts::TriggerModeV01::Passive)
        );
        assert!(
            current
                .accepts
                .as_ref()
                .is_some_and(|values| values.iter().any(|value| value == "value.core.message"))
        );
        assert!(
            current
                .message_keys
                .as_ref()
                .is_some_and(|policy| policy.accepted.iter().any(|key| key == "message"))
        );
    }
}
