use super::*;
use serde_json::{Map, json};
use skenion_contracts::{
    ObjectImplementationRefV01, ObjectProviderRefV01, ObjectResolutionStatusV01,
    ObjectResolutionV01,
};
use std::{env, fs, path::PathBuf};

fn assert_kind(resolution: &ObjectSpecResolution, kind: &str) {
    assert!(resolution.ok(), "{resolution:?}");
    assert_eq!(
        resolution
            .implementation
            .as_ref()
            .map(crate::current_node_identity::implementation_executable_kind)
            .as_deref(),
        Some(kind)
    );
}

fn assert_issue(resolution: &ObjectSpecResolution, code: &str) {
    assert_eq!(resolution.implementation, None);
    assert_eq!(resolution.issues[0].code, code);
}

fn test_implementation(object_id: &str) -> ObjectImplementationRefV01 {
    ObjectImplementationRefV01 {
        provider: ObjectProviderRefV01::Package {
            package_id: "test/package".to_owned(),
            lock_entry_id: None,
            version: Some(CURRENT_KIND_VERSION.to_owned()),
        },
        object_id: object_id.to_owned(),
        interface_digest: None,
    }
}

fn candidate(
    id: &str,
    source: ObjectRegistrySource,
    alias: &str,
    object_id: &str,
    display_name: &str,
) -> ObjectRegistryCandidate {
    ObjectRegistryCandidate {
        id: id.to_owned(),
        source,
        aliases: vec![alias.to_owned()],
        implementation: test_implementation(object_id),
        executable_kind: object_id.to_owned(),
        display_name: display_name.to_owned(),
        core: None,
        catalog_category: None,
        project_patch: None,
        package: None,
    }
}

fn temp_package_dir(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!(
        "skenion-runtime-object-spec-package-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn package_definition_json(id: &str) -> serde_json::Value {
    json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": id,
      "version": "0.1.0",
      "displayName": "Package Thing",
      "category": "Package",
      "ports": [
        { "id": "in", "direction": "input", "type": "value.core.float32", "rate": "control" },
        { "id": "out", "direction": "output", "type": "value.core.float32", "rate": "control" }
      ],
      "execution": { "model": "control" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    })
}

fn package_registry_with_object(
    package_dir: PathBuf,
    object_id: &str,
    primary_spec: &str,
) -> crate::PackageRegistryListResponseV01 {
    crate::PackageRegistryListResponseV01 {
        ok: true,
        packages: vec![crate::PackageRegistryEntryV01 {
            package_id: "example/package".to_owned(),
            version: "0.56.0".to_owned(),
            category: skenion_contracts::PackageCategoryV01::Mixed,
            source: skenion_contracts::PackageSourceV01::Workspace,
            root: skenion_contracts::PackageRootKindV01::Package,
            trust: skenion_contracts::PackageTrustV01::Trusted,
            contracts: skenion_contracts::PackageContractsRequirementV01 {
                version: skenion_contracts::CONTRACTS_PACKAGE_VERSION.to_owned(),
            },
            runtime_abi_range: None,
            targets: Vec::new(),
            manifest_path: crate::RUNTIME_PACKAGE_MANIFEST_FILE.to_owned(),
            root_path: Some(package_dir),
            manifest_checksum: zero_catalog_revision_checksum(),
            provides: skenion_contracts::PackageProvidesV01 {
                objects: vec![skenion_contracts::PackageObjectExportV01 {
                    object_id: object_id.to_owned(),
                    primary_object_spec: primary_spec.to_owned(),
                    aliases: vec![format!("{primary_spec}.alias")],
                    definition_path: "nodes/thing.json".to_owned(),
                    description: Some("A package object".to_owned()),
                    help_id: Some("example.package.thing".to_owned()),
                }],
                ..Default::default()
            },
            issues: Vec::new(),
        }],
        issues: Vec::new(),
    }
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
                    "implementation": {
                        "provider": { "kind": "core" },
                        "objectId": "inlet"
                    },
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
                    "implementation": {
                        "provider": { "kind": "core" },
                        "objectId": "outlet"
                    },
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
    assert_kind(&add, "object.core.operator.add");
    assert_eq!(add.params["right"], json!(1000.0));
    assert_eq!(add.instance_ports[0].id, "in");

    let sqrt = resolve_object_spec_v01("sqrt 2");
    assert_eq!(sqrt.issues[0].code, "object-spec.invalid-arg-count");

    let invalid = resolve_object_spec_v01("+ true");
    assert_eq!(invalid.issues[0].code, "object-spec.invalid-arg-type");

    for (input, kind, param, value) in [
        ("- -2", "object.core.operator.sub", "right", json!(-2)),
        ("/ 4", "object.core.operator.div", "right", json!(4)),
        ("* 3", "object.core.operator.mul", "right", json!(3)),
        ("pow 2", "object.core.operator.pow", "right", json!(2)),
        ("max 8", "object.core.operator.max", "right", json!(8)),
        ("min 1", "object.core.operator.min", "right", json!(1)),
    ] {
        let resolution = resolve_object_spec_v01(input);
        assert_kind(&resolution, kind);
        assert_eq!(resolution.params[param], value);
        assert_eq!(resolution.instance_ports.len(), 3);
        assert_eq!(resolution.instance_ports[0].port_type, "value.core.message");
        let accepts = resolution.instance_ports[0]
            .accepts
            .as_ref()
            .expect("operator hot inlet should publish numeric accepts");
        assert!(accepts.contains(&"value.core.float32".to_owned()));
        assert!(accepts.contains(&"value.core.int32".to_owned()));
        assert_eq!(resolution.instance_ports[2].port_type, "value.core.int32");
    }

    let float_mul = resolve_object_spec_v01("* 3.");
    assert_kind(&float_mul, "object.core.operator.mul");
    assert_eq!(float_mul.display_text, "* 3.0");
    assert_eq!(float_mul.params["right"], json!(3.0));
    assert_eq!(float_mul.instance_ports[0].port_type, "value.core.message");
    assert_eq!(float_mul.instance_ports[1].port_type, "value.core.message");
    assert_eq!(float_mul.instance_ports[2].port_type, "value.core.float32");

    let sqrt = resolve_object_spec_v01("sqrt");
    assert_kind(&sqrt, "object.core.operator.sqrt");
    assert_eq!(sqrt.instance_ports.len(), 2);
    assert_eq!(sqrt.instance_ports[0].port_type, "value.core.message");
    assert_eq!(sqrt.instance_ports[1].port_type, "value.core.float32");

    let default_add = resolve_object_spec_v01("object.core.operator.add");
    assert_kind(&default_add, "object.core.operator.add");
    assert_eq!(default_add.params["right"], json!(0.0));

    assert_issue(
        &resolve_object_spec_v01("sqrt 1"),
        "object-spec.invalid-arg-count",
    );
    assert_issue(
        &resolve_object_spec_v01("+ 1 2"),
        "object-spec.invalid-arg-count",
    );
    assert_issue(
        &resolve_object_spec_v01("object.core.operator.mul false"),
        "object-spec.invalid-arg-type",
    );
}

#[test]
fn resolves_runtime_value_audio_and_subpatch_aliases() {
    let float = resolve_object_spec_v01("f 0.25");
    assert!(float.ok());
    assert_kind(&float, "object.core.float");
    assert_eq!(float.params["value"], json!(0.25));

    let osc = resolve_object_spec_v01("osc~ 220");
    assert!(osc.ok());
    assert_kind(&osc, "object.core.audio.osc");
    assert_eq!(osc.params["frequency"], json!(220.0));

    let mul = resolve_object_spec_v01("*~");
    assert!(mul.ok());
    assert_kind(&mul, "object.core.audio.operator.mul");
    assert_eq!(mul.instance_ports.len(), 3);

    let scalar_mul = resolve_object_spec_v01("*~ 0.5");
    assert_eq!(scalar_mul.issues[0].code, "object-spec.invalid-arg-count");

    let unsupported = resolve_object_spec_v01("+~");
    assert_eq!(
        unsupported.issues[0].code,
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
        assert_issue(
            &resolve_object_spec_v01(input),
            "object-spec.unsupported-first-party",
        );
    }

    let sig = resolve_object_spec_v01("sig~");
    assert_kind(&sig, "object.core.audio.sig");
    assert_eq!(sig.params["value"], json!(0.0));

    let invalid_sig = resolve_object_spec_v01("sig~ false");
    assert_issue(&invalid_sig, "object-spec.invalid-arg-type");
    assert_issue(
        &resolve_object_spec_v01("sig~ 1 2"),
        "object-spec.invalid-arg-count",
    );

    let osc = resolve_object_spec_v01("object.core.audio.osc 220");
    assert_kind(&osc, "object.core.audio.osc");
    assert_eq!(osc.params["frequency"], json!(220.0));
    assert_issue(
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
    assert_issue(&invalid_audio_output, "object-spec.invalid-arg-count");

    let subpatch = resolve_object_spec_v01("p voice");
    assert!(subpatch.ok());
    assert_kind(&subpatch, "object.core.subpatch");
    assert_eq!(subpatch.params["patchRef"], json!("voice"));
}

#[test]
fn resolves_runtime_value_boxes_and_boundary_aliases() {
    for (input, kind, representation, value, port_type) in [
        (
            "float",
            "object.core.float",
            "f32",
            json!(0.0),
            "value.core.float32",
        ),
        (
            "float ufloat16 1.5",
            "object.core.float",
            "ufloat16",
            json!(1.5),
            "value.core.ufloat16",
        ),
        (
            "int -7",
            "object.core.int",
            "i32",
            json!(-7),
            "value.core.int32",
        ),
        (
            "int u32 9",
            "object.core.int",
            "u32",
            json!(9),
            "value.core.uint32",
        ),
        (
            "int u8 255",
            "object.core.int",
            "u8",
            json!(255),
            "value.core.uint8",
        ),
    ] {
        let resolution = resolve_object_spec_v01(input);
        assert_kind(&resolution, kind);
        assert_eq!(resolution.params["representation"], json!(representation));
        assert_eq!(resolution.params["value"], value);
        assert_eq!(resolution.instance_ports[1].port_type, port_type);
        assert_eq!(resolution.instance_ports[2].port_type, port_type);
        assert_eq!(resolution.instance_ports.len(), 3);
    }

    assert_issue(
        &resolve_object_spec_v01("int 1.5"),
        "object-spec.invalid-arg-type",
    );
    assert_issue(
        &resolve_object_spec_v01("int u8 -1"),
        "object-spec.invalid-arg-type",
    );
    assert_issue(
        &resolve_object_spec_v01("float 1 2"),
        "object-spec.invalid-arg-type",
    );
    assert_issue(&resolve_object_spec_v01("uint 9"), "object-spec.unresolved");

    let bang = resolve_object_spec_v01("bang");
    assert_kind(&bang, "object.core.bang");
    assert!(bang.params.is_empty());
    assert_eq!(bang.instance_ports[1].port_type, "value.core.bang");
    assert_issue(
        &resolve_object_spec_v01("object.core.bang 1"),
        "object-spec.invalid-arg-count",
    );

    let float_alias = resolve_object_spec_v01("f 1.5");
    assert_kind(&float_alias, "object.core.float");
    assert_eq!(float_alias.params["value"], json!(1.5));
    assert_issue(
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

    assert_issue(
        &resolve_object_spec_v01("p"),
        "object-spec.invalid-arg-count",
    );
    assert_issue(
        &resolve_object_spec_v01("p true"),
        "object-spec.invalid-arg-type",
    );
    assert_issue(
        &resolve_object_spec_v01("inlet left right"),
        "object-spec.invalid-arg-count",
    );
    assert_issue(
        &resolve_object_spec_v01("outlet 1"),
        "object-spec.invalid-arg-type",
    );
}

#[test]
fn core_catalog_uses_readable_primary_object_specs() {
    let snapshot = ObjectRegistry::first_party_core().catalog_projection();

    for (object_id, primary_object_spec) in [
        ("bang", "bang"),
        ("message", "message"),
        ("float", "float"),
        ("int", "int"),
    ] {
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.object_id == object_id)
            .expect("core object should appear in catalog");
        assert_eq!(entry.primary_object_spec, primary_object_spec);
    }
    assert!(
        snapshot
            .entries
            .iter()
            .all(|entry| entry.object_id != "uint")
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
        assert_eq!(resolution.implementation, None);
        assert_eq!(resolution.issues[0].code, "object-spec.payload-identity");
    }
}

#[test]
fn reports_unresolved_and_syntax_issues_without_runtime_mapping() {
    let unresolved = resolve_object_spec_v01("user.manipulator 1");
    assert_eq!(unresolved.issues[0].code, "object-spec.unresolved");

    let invalid = resolve_object_spec_v01("[+ 1");
    assert_eq!(invalid.issues[0].code, "object-spec.invalid-syntax");

    let empty = resolve_object_spec_v01("   ");
    assert_eq!(empty.issues[0].code, "object-spec.empty");

    assert_eq!(
        runtime_object_spec_issue_code("object-spec.custom-parser-code"),
        "object-spec.custom-parser-code"
    );
    assert_eq!(
        runtime_object_spec_issue_code("custom-parser-code"),
        "object-spec.custom-parser-code"
    );
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
                &entry.provider,
                ObjectProviderRefV01::ProjectPatch { patch_id, .. }
                    if patch_id == "my-patcher"
            )
        })
        .expect("project patch should appear in catalog");

    assert_eq!(project_entry.catalog_id, "project.my-patcher");
    assert_eq!(project_entry.primary_object_spec, "my-patcher");
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
    let explicit_canonical = registry.resolve("object.core.subpatch my-patcher");
    assert_kind(&explicit_canonical, "object.project.patch.my-patcher");

    assert_issue(
        &registry.resolve("my-patcher 1"),
        "object-spec.invalid-arg-count",
    );
    assert_issue(&registry.resolve("p"), "object-spec.invalid-arg-count");
    assert_issue(&registry.resolve("p true"), "object-spec.invalid-arg-type");

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
            implementation: ObjectImplementationRefV01 {
                provider: ObjectProviderRefV01::ProjectPatch {
                    patch_id: "my-patcher".to_owned(),
                    revision: Some("7".to_owned()),
                    interface_revision: None,
                    interface_digest: Some(skenion_contracts::compute_patch_interface_digest_v01(
                        &patch,
                    )),
                },
                object_id: "my-patcher".to_owned(),
                interface_digest: Some(skenion_contracts::compute_patch_interface_digest_v01(
                    &patch,
                )),
            },
            executable_kind: project_patch_object_kind("my-patcher"),
            display_name: "Patch my-patcher".to_owned(),
            core: None,
            catalog_category: None,
            project_patch: Some(ProjectPatchCandidate {
                patch_id: "my-patcher".to_owned(),
                revision: "7".to_owned(),
                description: Some("my-patcher reusable patch".to_owned()),
                interface_digest: skenion_contracts::compute_patch_interface_digest_v01(&patch),
                ports: project_patch_ports(&patch),
            }),
            package: None,
        },
    );
    assert_issue(&mismatched, "object-spec.unresolved");
}

#[test]
fn object_spec_parser_preserves_runtime_atom_boundaries() {
    let large_uint =
        contract_object_spec_atom_to_runtime(&skenion_contracts::ObjectSpecAtomV01::Uint {
            value: u64::MAX,
            representation: None,
        });
    assert_eq!(large_uint, ObjectSpecAtom::Symbol(u64::MAX.to_string()));
    let in_range_uint =
        contract_object_spec_atom_to_runtime(&skenion_contracts::ObjectSpecAtomV01::Uint {
            value: i64::MAX as u64,
            representation: None,
        });
    assert_eq!(in_range_uint, ObjectSpecAtom::Int(i64::MAX));

    assert_issue(
        &resolve_object_spec_v01("int false"),
        "object-spec.invalid-arg-type",
    );
    assert_issue(
        &resolve_object_spec_v01("int u8 false"),
        "object-spec.invalid-arg-type",
    );
}

#[test]
fn reserved_providers_and_unconstructable_candidates_fail_closed() {
    let provider_registry = ObjectRegistry {
        candidates: vec![candidate(
            "package:vendor.node",
            ObjectRegistrySource::PackageProvider,
            "vendor.node",
            "object.vendor.node",
            "Vendor Node",
        )],
        allow_unchecked_project_patch_refs: false,
    };
    let unavailable_provider = provider_registry.resolve("vendor.node");
    assert_eq!(
        unavailable_provider
            .implementation
            .as_ref()
            .map(|implementation| implementation.object_id.as_str()),
        Some("object.vendor.node")
    );
    assert_eq!(
        unavailable_provider.object_resolution.status,
        ObjectResolutionStatusV01::Error
    );
    assert_eq!(
        unavailable_provider.issues[0].code,
        "object-spec.provider-unavailable"
    );

    let ambiguous_registry = ObjectRegistry {
        candidates: vec![
            candidate(
                "package:shared.node",
                ObjectRegistrySource::PackageProvider,
                "shared.node",
                "object.vendor.shared",
                "Package Shared",
            ),
            candidate(
                "native:shared.node",
                ObjectRegistrySource::NativeProvider,
                "shared.node",
                "object.native.shared",
                "Native Shared",
            ),
        ],
        allow_unchecked_project_patch_refs: false,
    };
    let ambiguous = ambiguous_registry.resolve("shared.node");
    assert_issue(&ambiguous, "object-spec.ambiguous");
    assert_eq!(ambiguous.candidates[0].source, "package-provider");
    assert_eq!(ambiguous.candidates[1].source, "native-provider");

    let missing_patch_metadata = ObjectRegistry {
        candidates: vec![ObjectRegistryCandidate {
            id: "project-patch:broken".to_owned(),
            source: ObjectRegistrySource::ProjectPatch,
            aliases: vec!["broken".to_owned()],
            implementation: test_implementation("broken"),
            executable_kind: project_patch_object_kind("broken"),
            display_name: "Broken".to_owned(),
            core: None,
            catalog_category: None,
            project_patch: None,
            package: None,
        }],
        allow_unchecked_project_patch_refs: false,
    };
    assert_issue(
        &missing_patch_metadata.resolve("broken"),
        "object-spec.unresolved",
    );

    let core_without_constructor = ObjectRegistry {
        candidates: vec![ObjectRegistryCandidate {
            id: "object.core.future".to_owned(),
            source: ObjectRegistrySource::FirstPartyCore,
            aliases: vec!["future".to_owned()],
            implementation: test_implementation("future"),
            executable_kind: "object.core.future".to_owned(),
            display_name: "Future".to_owned(),
            core: None,
            catalog_category: Some("Core"),
            project_patch: None,
            package: None,
        }],
        allow_unchecked_project_patch_refs: false,
    };
    assert_issue(
        &core_without_constructor.resolve("future"),
        "object-spec.unresolved",
    );
}

#[test]
fn package_objects_project_to_catalog_and_resolve_from_installed_registry() {
    let package_dir = temp_package_dir("catalog");
    fs::create_dir_all(package_dir.join("nodes")).unwrap();
    fs::write(
        package_dir.join("nodes/thing.json"),
        serde_json::to_vec(&package_definition_json("example.package.thing")).unwrap(),
    )
    .unwrap();
    let packages = package_registry_with_object(package_dir, "example.package.thing", "thing");
    let registry = ObjectRegistry::for_project_with_packages(None, Some(&packages));

    let snapshot = registry.catalog_projection();
    let entry = snapshot
        .entries
        .iter()
        .find(|entry| entry.object_id == "example.package.thing")
        .expect("package object should appear in node catalog");
    assert_eq!(entry.primary_object_spec, "thing");
    assert_eq!(
        entry.aliases.as_deref(),
        Some(&["thing.alias".to_owned()][..])
    );
    assert!(matches!(
        &entry.provider,
        ObjectProviderRefV01::Package {
            package_id,
            version,
            ..
        } if package_id == "example/package" && version.as_deref() == Some("0.56.0")
    ));
    assert_eq!(entry.definition.id, "example.package.thing");
    assert_eq!(entry.definition.ports.len(), 2);

    let resolved = registry.resolve("thing");
    assert!(resolved.ok(), "{resolved:?}");
    assert_eq!(
        resolved
            .implementation
            .as_ref()
            .map(|implementation| implementation.object_id.as_str()),
        Some("example.package.thing")
    );
    assert!(matches!(
        resolved
            .implementation
            .as_ref()
            .map(|implementation| &implementation.provider),
        Some(ObjectProviderRefV01::Package { version, .. }) if version.as_deref() == Some("0.56.0")
    ));
    assert_eq!(
        resolved.object_resolution.status,
        ObjectResolutionStatusV01::Resolved
    );
    assert_eq!(resolved.instance_ports.len(), 2);
}

#[test]
fn object_spec_materialization_and_port_projection_cover_issue_edges() {
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
        implementation: None,
        object_resolution: ObjectResolutionV01 {
            status: ObjectResolutionStatusV01::Unresolved,
            selected_spec: None,
            candidates: Vec::new(),
            issues: Vec::new(),
        },
        params: Map::new(),
        instance_ports: Vec::new(),
        candidates: vec![ObjectSpecCandidateSummary {
            id: "package:future".to_owned(),
            source: "package-provider".to_owned(),
            implementation: test_implementation("object.future"),
            object_spec: Some("future".to_owned()),
            display_name: "Future".to_owned(),
        }],
        issues: Vec::new(),
    };

    let materialize_error = materialize_object_spec_node_v01(&unresolved, "future_1")
        .expect_err("unresolved object should not materialize as resolved node");
    assert_eq!(materialize_error.code, "object-spec.unresolved");
    assert!(materialize_error.message.contains("future"));

    let issue_node = materialize_unresolved_object_spec_node_v01(&unresolved, "future_1");
    assert_eq!(issue_node.implementation, None);
    assert_eq!(
        issue_node.object_resolution.as_ref().unwrap().status,
        ObjectResolutionStatusV01::Unresolved
    );
    assert_eq!(issue_node.params["candidateCount"], json!(1));
    assert_eq!(
        issue_node.params["candidates"][0]["source"],
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
        let accepts = current.accepts.as_ref().expect("message input accepts");
        for (index, value) in accepts.iter().enumerate() {
            assert!(
                !accepts[index + 1..]
                    .iter()
                    .any(|candidate| candidate == value),
                "duplicate accepts entry {value}"
            );
        }
        assert!(
            current
                .message_keys
                .as_ref()
                .is_some_and(|policy| policy.accepted.iter().any(|key| key == "message"))
        );
    }
}
