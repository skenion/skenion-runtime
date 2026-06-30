use serde_json::{Map, Value, json};
use skenion_contracts::{
    NodeCatalogDiagnosticNodeDefinitionReasonV01, NodeCatalogDiagnosticNodeDefinitionV01,
    NodeCatalogDisplayPaletteV01, NodeCatalogDisplayV01, NodeCatalogEntryV01,
    NodeCatalogSnapshotV01, NodeCatalogSourceV01, PackageChecksumAlgorithmV01, PackageChecksumV01,
};

use crate::{
    NodeDefinitionCurrent, PatchDefinitionCurrent, PortDirectionCurrent, PortRateCurrent,
    PortSpecCurrent, ProjectDocumentCurrent,
    nodes::{CoreNodeConstructor, CoreNodeImplementation, first_party_core_nodes},
};

mod ports;
mod projection;
mod types;

pub(crate) use types::{
    ObjectRegistry, ObjectSpecAtom, ObjectSpecCandidateSummary, ObjectSpecDiagnostic,
    ObjectSpecPort, ObjectSpecPortActivation, ObjectSpecPortDirection, ObjectSpecPortRate,
    ObjectSpecResolution,
};

use types::{
    ObjectRegistryCandidate, ObjectRegistrySource, ParsedObjectSpec, ProjectPatchCandidate,
};

const CURRENT_KIND_VERSION: &str = "0.1.0";
pub(crate) const PROJECT_PATCH_OBJECT_KIND_PREFIX: &str = "object.project.patch.";

impl ObjectRegistry {
    pub(crate) fn first_party_core() -> Self {
        let mut registry = Self {
            candidates: Vec::new(),
            allow_unchecked_project_patch_refs: false,
        };
        registry.register_first_party_core();
        registry
    }

    pub(crate) fn for_project(project: Option<&ProjectDocumentCurrent>) -> Self {
        Self::for_patch_library(project.map_or(&[], |project| project.patch_library.as_slice()))
    }

    pub(crate) fn for_patch_library(patch_library: &[PatchDefinitionCurrent]) -> Self {
        let mut registry = Self::first_party_core();
        registry.register_project_patches(patch_library);
        registry
    }

    fn allow_unchecked_project_patch_refs(mut self) -> Self {
        self.allow_unchecked_project_patch_refs = true;
        self
    }

    pub(crate) fn resolve(&self, input: &str) -> ObjectSpecResolution {
        let parsed = match parse_object_spec_input_v01(input) {
            Ok(parsed) => parsed,
            Err(resolution) => return *resolution,
        };

        if is_payload_identity_kind(&parsed.class_symbol) {
            return failure(
                &parsed.input,
                parsed.display_text,
                &parsed.class_symbol,
                parsed.creation_args,
                "object-spec.payload-identity",
                format!(
                    "{} is a payload identity, not an executable object",
                    parsed.class_symbol
                ),
            );
        }

        if let Some(message) = unsupported_first_party_audio_message(&parsed.class_symbol) {
            return failure(
                &parsed.input,
                parsed.display_text,
                &parsed.class_symbol,
                parsed.creation_args,
                "object-spec.unsupported-first-party",
                message,
            );
        }

        let candidates = self.lookup_candidates(&parsed);
        match candidates.len() {
            0 => unresolved_resolution(parsed),
            1 => self.construct_candidate(parsed, &candidates[0]),
            _ => ambiguous_resolution(parsed, candidates),
        }
    }

    pub(crate) fn catalog_projection(&self) -> NodeCatalogSnapshotV01 {
        let mut entries =
            self.candidates
                .iter()
                .filter_map(|candidate| match candidate.source {
                    ObjectRegistrySource::FirstPartyCore => self.core_catalog_entry(candidate),
                    ObjectRegistrySource::ProjectPatch => project_patch_catalog_entry(candidate),
                    ObjectRegistrySource::PackageProvider
                    | ObjectRegistrySource::NativeProvider => None,
                })
                .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.catalog_id.cmp(&right.catalog_id));

        let mut snapshot = NodeCatalogSnapshotV01 {
            schema: "skenion.node-catalog.snapshot".to_owned(),
            schema_version: CURRENT_KIND_VERSION.to_owned(),
            catalog_revision: zero_catalog_revision_checksum(),
            entries,
            diagnostic_node_definitions: vec![NodeCatalogDiagnosticNodeDefinitionV01 {
                diagnostic_id: "runtime.unresolved-object".to_owned(),
                reason: NodeCatalogDiagnosticNodeDefinitionReasonV01::UnresolvedObject,
                definition: unresolved_object_spec_node_definition_v01(),
            }],
            diagnostics: None,
        };
        snapshot.catalog_revision = skenion_contracts::compute_node_catalog_revision_v01(&snapshot);
        snapshot
    }

    pub(crate) fn node_definition_projection(&self) -> Vec<NodeDefinitionCurrent> {
        let snapshot = self.catalog_projection();
        let mut definitions = snapshot
            .entries
            .into_iter()
            .map(|entry| entry.definition)
            .collect::<Vec<_>>();
        definitions.extend(
            snapshot
                .diagnostic_node_definitions
                .into_iter()
                .map(|definition| definition.definition),
        );
        definitions
    }

    fn core_catalog_entry(
        &self,
        candidate: &ObjectRegistryCandidate,
    ) -> Option<NodeCatalogEntryV01> {
        if candidate.kind == "object.core.subpatch" {
            return None;
        }

        let canonical_object_spec = candidate.canonical_object_spec()?;
        let resolution = self.resolve(&canonical_object_spec);
        if !resolution.ok() {
            return None;
        }
        let mut definition = object_spec_node_definition_v01(&resolution)?;
        definition.display_name = candidate.display_name.clone();
        definition.category = core_catalog_category(candidate).to_owned();

        Some(NodeCatalogEntryV01 {
            catalog_id: catalog_id_for_core_candidate(candidate),
            canonical_object_spec,
            aliases: None,
            source: NodeCatalogSourceV01::Core,
            definition,
            creatable: true,
            display: NodeCatalogDisplayV01 {
                title: candidate.display_name.clone(),
                category: Some(core_catalog_category(candidate).to_owned()),
                palette: Some(NodeCatalogDisplayPaletteV01::Text),
                description: None,
                help_id: Some(candidate.kind.clone()),
            },
            diagnostics: None,
        })
    }

    fn register_first_party_core(&mut self) {
        for node in first_party_core_nodes() {
            self.register_core_candidate(*node);
        }
    }

    fn register_core_candidate(&mut self, node: &'static dyn CoreNodeImplementation) {
        self.candidates.push(ObjectRegistryCandidate {
            id: node.kind().to_owned(),
            source: ObjectRegistrySource::FirstPartyCore,
            aliases: node
                .aliases()
                .iter()
                .map(|alias| (*alias).to_owned())
                .collect(),
            kind: node.kind().to_owned(),
            kind_version: CURRENT_KIND_VERSION.to_owned(),
            display_name: node.display_name().to_owned(),
            constructor: Some(node.constructor()),
            catalog_category: Some(node.catalog_category()),
            project_patch: None,
        });
    }

    fn register_project_patches(&mut self, patch_library: &[PatchDefinitionCurrent]) {
        for patch in patch_library {
            let kind = project_patch_object_kind(&patch.id);
            self.candidates.push(ObjectRegistryCandidate {
                id: format!("project-patch:{}", patch.id),
                source: ObjectRegistrySource::ProjectPatch,
                aliases: vec![patch.id.clone()],
                kind,
                kind_version: CURRENT_KIND_VERSION.to_owned(),
                display_name: patch
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.title.clone())
                    .unwrap_or_else(|| patch.id.clone()),
                constructor: None,
                catalog_category: None,
                project_patch: Some(ProjectPatchCandidate {
                    patch_id: patch.id.clone(),
                    revision: patch.revision.clone(),
                    description: patch
                        .metadata
                        .as_ref()
                        .and_then(|metadata| metadata.description.clone()),
                    interface_digest: skenion_contracts::compute_patch_interface_digest_v01(patch),
                    ports: project_patch_ports(patch),
                }),
            });
        }
    }

    fn lookup_candidates(&self, parsed: &ParsedObjectSpec) -> Vec<ObjectRegistryCandidate> {
        if matches!(parsed.class_symbol.as_str(), "p" | "object.core.subpatch") {
            return self.lookup_explicit_project_patch_candidates(parsed);
        }

        self.candidates
            .iter()
            .filter(|candidate| candidate.matches_class_symbol(&parsed.class_symbol))
            .cloned()
            .collect()
    }

    fn lookup_explicit_project_patch_candidates(
        &self,
        parsed: &ParsedObjectSpec,
    ) -> Vec<ObjectRegistryCandidate> {
        let Some(patch_id) = explicit_project_patch_ref(parsed) else {
            return self
                .core_candidate("object.core.subpatch")
                .into_iter()
                .collect();
        };

        let matches = self
            .candidates
            .iter()
            .filter(|candidate| {
                candidate.source == ObjectRegistrySource::ProjectPatch
                    && candidate
                        .project_patch
                        .as_ref()
                        .is_some_and(|patch| patch.patch_id == patch_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !matches.is_empty() || !self.allow_unchecked_project_patch_refs {
            return matches;
        }

        vec![ObjectRegistryCandidate {
            id: format!("project-patch:{patch_id}"),
            source: ObjectRegistrySource::ProjectPatch,
            aliases: vec![patch_id.clone()],
            kind: "object.core.subpatch".to_owned(),
            kind_version: CURRENT_KIND_VERSION.to_owned(),
            display_name: patch_id.clone(),
            constructor: Some(CoreNodeConstructor::Subpatch),
            catalog_category: Some("Core"),
            project_patch: Some(ProjectPatchCandidate {
                patch_id,
                revision: CURRENT_KIND_VERSION.to_owned(),
                description: None,
                interface_digest: zero_catalog_revision_checksum(),
                ports: Vec::new(),
            }),
        }]
    }

    fn core_candidate(&self, kind: &str) -> Option<ObjectRegistryCandidate> {
        self.candidates
            .iter()
            .find(|candidate| {
                candidate.source == ObjectRegistrySource::FirstPartyCore && candidate.kind == kind
            })
            .cloned()
    }

    fn construct_candidate(
        &self,
        parsed: ParsedObjectSpec,
        candidate: &ObjectRegistryCandidate,
    ) -> ObjectSpecResolution {
        match candidate.source {
            ObjectRegistrySource::FirstPartyCore => construct_first_party_core(parsed, candidate),
            ObjectRegistrySource::ProjectPatch => construct_project_patch(parsed, candidate),
            ObjectRegistrySource::PackageProvider | ObjectRegistrySource::NativeProvider => {
                failure(
                    &parsed.input,
                    parsed.display_text,
                    &parsed.class_symbol,
                    parsed.creation_args,
                    "object-spec.provider-unavailable",
                    "package and native object providers are reserved but not loaded in this Runtime tranche",
                )
            }
        }
    }
}

impl ObjectRegistryCandidate {
    fn matches_class_symbol(&self, class_symbol: &str) -> bool {
        self.aliases.iter().any(|alias| alias == class_symbol)
    }

    fn summary(&self) -> ObjectSpecCandidateSummary {
        ObjectSpecCandidateSummary {
            id: self.id.clone(),
            source: match self.source {
                ObjectRegistrySource::FirstPartyCore => "first-party-core",
                ObjectRegistrySource::ProjectPatch => "project-patch",
                ObjectRegistrySource::PackageProvider => "package-provider",
                ObjectRegistrySource::NativeProvider => "native-provider",
            }
            .to_owned(),
            kind: self.kind.clone(),
            display_name: self.display_name.clone(),
        }
    }

    fn canonical_object_spec(&self) -> Option<String> {
        self.aliases
            .iter()
            .find(|alias| !alias.starts_with("object."))
            .or_else(|| self.aliases.first())
            .cloned()
    }
}

fn project_patch_catalog_entry(candidate: &ObjectRegistryCandidate) -> Option<NodeCatalogEntryV01> {
    let patch = candidate.project_patch.as_ref()?;
    let definition = project_patch_catalog_definition(candidate, patch);
    Some(NodeCatalogEntryV01 {
        catalog_id: format!(
            "project.{}",
            skenion_contracts::sanitize_project_patch_id_v01(&patch.patch_id)
        ),
        canonical_object_spec: patch.patch_id.clone(),
        aliases: None,
        source: NodeCatalogSourceV01::ProjectPatch {
            patch_id: patch.patch_id.clone(),
            patch_revision: None,
            interface_digest: patch.interface_digest.clone(),
        },
        definition,
        creatable: true,
        display: NodeCatalogDisplayV01 {
            title: candidate.display_name.clone(),
            category: Some("Project Patch".to_owned()),
            palette: Some(NodeCatalogDisplayPaletteV01::Direct),
            description: patch.description.clone(),
            help_id: None,
        },
        diagnostics: None,
    })
}

fn project_patch_catalog_definition(
    candidate: &ObjectRegistryCandidate,
    patch: &ProjectPatchCandidate,
) -> NodeDefinitionCurrent {
    let ports = patch
        .ports
        .iter()
        .map(object_spec_port_to_current)
        .collect::<Vec<_>>();
    let has_audio_port = ports
        .iter()
        .any(|port| port.rate == Some(PortRateCurrent::Audio));

    NodeDefinitionCurrent {
        schema: "skenion.node.definition".to_owned(),
        schema_version: CURRENT_KIND_VERSION.to_owned(),
        id: skenion_contracts::project_patch_node_definition_id_v01(
            &patch.patch_id,
            &patch.interface_digest,
        ),
        version: CURRENT_KIND_VERSION.to_owned(),
        display_name: candidate.display_name.clone(),
        category: "Project Patch".to_owned(),
        script_api_version: None,
        bundle_hash: None,
        surface: None,
        ports,
        port_groups: None,
        execution: skenion_contracts::NodeExecutionV01 {
            model: if has_audio_port {
                skenion_contracts::ExecutionModelV01::AudioBlock
            } else {
                skenion_contracts::ExecutionModelV01::Control
            },
            clock: None,
        },
        state: skenion_contracts::NodeStateV01 { persistent: false },
        permissions: Vec::new(),
        capabilities: Vec::new(),
    }
}

fn core_catalog_category(candidate: &ObjectRegistryCandidate) -> &'static str {
    candidate.catalog_category.unwrap_or("Core")
}

fn catalog_id_for_core_candidate(candidate: &ObjectRegistryCandidate) -> String {
    let suffix = candidate
        .kind
        .strip_prefix("object.core.")
        .unwrap_or(candidate.kind.as_str());
    format!("core.{suffix}")
}

fn zero_catalog_revision_checksum() -> PackageChecksumV01 {
    PackageChecksumV01 {
        algorithm: PackageChecksumAlgorithmV01::Sha256,
        value: "0".repeat(64),
    }
}

pub(crate) fn resolve_object_spec_v01(input: &str) -> ObjectSpecResolution {
    ObjectRegistry::first_party_core()
        .allow_unchecked_project_patch_refs()
        .resolve(input)
}

fn parse_object_spec_input_v01(input: &str) -> Result<ParsedObjectSpec, Box<ObjectSpecResolution>> {
    let parsed = skenion_contracts::parse_object_spec_v01(input);
    let creation_args = parsed
        .creation_args
        .iter()
        .map(contract_object_spec_atom_to_runtime)
        .collect::<Vec<_>>();
    if parsed.ok {
        return Ok(ParsedObjectSpec {
            input: parsed.input,
            display_text: parsed.display_text,
            class_symbol: parsed.class_name,
            creation_args,
        });
    }

    let diagnostic = parsed.diagnostics.first();
    let code = diagnostic
        .map(|diagnostic| runtime_object_spec_diagnostic_code(&diagnostic.code))
        .unwrap_or_else(|| "object-spec.invalid-syntax".to_owned());
    let message = diagnostic
        .map(|diagnostic| diagnostic.message.clone())
        .unwrap_or_else(|| "object spec could not be parsed".to_owned());
    Err(Box::new(failure(
        &parsed.input,
        parsed.display_text,
        &parsed.class_name,
        creation_args,
        code,
        message,
    )))
}

fn runtime_object_spec_diagnostic_code(code: &str) -> String {
    match code {
        "empty-object-spec" => "object-spec.empty".to_owned(),
        "invalid-syntax" => "object-spec.invalid-syntax".to_owned(),
        value if value.starts_with("object-spec.") => value.to_owned(),
        value => format!("object-spec.{value}"),
    }
}

fn contract_object_spec_atom_to_runtime(
    atom: &skenion_contracts::ObjectSpecAtomV01,
) -> ObjectSpecAtom {
    match atom {
        skenion_contracts::ObjectSpecAtomV01::Float { value, .. } => ObjectSpecAtom::Float(*value),
        skenion_contracts::ObjectSpecAtomV01::Int { value, .. } => ObjectSpecAtom::Int(*value),
        skenion_contracts::ObjectSpecAtomV01::Uint { value, .. } => {
            if *value <= i64::MAX as u64 {
                ObjectSpecAtom::Int(*value as i64)
            } else {
                ObjectSpecAtom::Symbol(value.to_string())
            }
        }
        skenion_contracts::ObjectSpecAtomV01::Bool { value } => ObjectSpecAtom::Bool(*value),
        skenion_contracts::ObjectSpecAtomV01::Identifier { value }
        | skenion_contracts::ObjectSpecAtomV01::String { value } => {
            ObjectSpecAtom::Symbol(value.clone())
        }
    }
}

fn construct_first_party_core(
    parsed: ParsedObjectSpec,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let ParsedObjectSpec {
        input,
        display_text,
        class_symbol,
        creation_args,
    } = parsed;

    match candidate.constructor {
        Some(CoreNodeConstructor::ControlOperator) => {
            return resolve_control_operator(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                candidate,
            );
        }
        Some(CoreNodeConstructor::ControlValue) => {
            return resolve_control_value(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                candidate,
            );
        }
        Some(CoreNodeConstructor::Audio) => {
            return resolve_audio_object(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                candidate,
            );
        }
        Some(CoreNodeConstructor::Subpatch) => {
            return resolve_named_ref_object(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                candidate,
                "patchRef",
                "subpatch object spec requires exactly one patch reference",
            );
        }
        Some(CoreNodeConstructor::BoundaryPort) => {
            return resolve_optional_named_ref_object(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                candidate,
                "portId",
            );
        }
        None => {}
    }

    failure_with_candidates(
        &input,
        display_text,
        &class_symbol,
        creation_args,
        vec![candidate.summary()],
        "object-spec.unresolved",
        format!(
            "{} is registered but has no Runtime constructor",
            candidate.kind
        ),
    )
}

fn construct_project_patch(
    parsed: ParsedObjectSpec,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let ParsedObjectSpec {
        input,
        display_text,
        class_symbol,
        creation_args,
    } = parsed;
    let Some(patch) = candidate.project_patch.as_ref() else {
        return failure_with_candidates(
            &input,
            display_text,
            &class_symbol,
            creation_args,
            vec![candidate.summary()],
            "object-spec.unresolved",
            "project patch candidate is missing patch metadata",
        );
    };

    if matches!(class_symbol.as_str(), "p" | "object.core.subpatch") {
        if creation_args.len() != 1 {
            return failure_with_candidates(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                vec![candidate.summary()],
                "object-spec.invalid-arg-count",
                "subpatch object spec requires exactly one patch reference",
            );
        }
        let Some(reference) = symbol_value(&creation_args[0]) else {
            return failure_with_candidates(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                vec![candidate.summary()],
                "object-spec.invalid-arg-type",
                format!("{class_symbol} reference argument must be a symbol"),
            );
        };
        if reference != patch.patch_id {
            return failure_with_candidates(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                vec![candidate.summary()],
                "object-spec.unresolved",
                format!("project patch {reference} is not available in the active project"),
            );
        }
    } else if !creation_args.is_empty() {
        return failure_with_candidates(
            &input,
            display_text,
            &class_symbol,
            creation_args,
            vec![candidate.summary()],
            "object-spec.invalid-arg-count",
            format!("{class_symbol} project patch shortcut accepts no creation arguments"),
        );
    }

    let mut params = Map::new();
    params.insert("patchRef".to_owned(), Value::String(patch.patch_id.clone()));
    params.insert(
        "patchRevision".to_owned(),
        Value::String(patch.revision.clone()),
    );
    success(
        &input,
        display_text,
        &class_symbol,
        creation_args,
        candidate,
        params,
        patch.ports.clone(),
    )
}

fn explicit_project_patch_ref(parsed: &ParsedObjectSpec) -> Option<String> {
    if parsed.creation_args.len() != 1 {
        return None;
    }
    symbol_value(&parsed.creation_args[0])
}

fn unresolved_resolution(parsed: ParsedObjectSpec) -> ObjectSpecResolution {
    failure(
        &parsed.input,
        parsed.display_text,
        &parsed.class_symbol,
        parsed.creation_args,
        "object-spec.unresolved",
        format!(
            "{} is not available in the local Runtime object registry",
            parsed.class_symbol
        ),
    )
}

fn ambiguous_resolution(
    parsed: ParsedObjectSpec,
    candidates: Vec<ObjectRegistryCandidate>,
) -> ObjectSpecResolution {
    let summaries = candidates
        .iter()
        .map(ObjectRegistryCandidate::summary)
        .collect::<Vec<_>>();
    let candidate_list = summaries
        .iter()
        .map(|candidate| format!("{} ({})", candidate.id, candidate.source))
        .collect::<Vec<_>>()
        .join(", ");
    failure_with_candidates(
        &parsed.input,
        parsed.display_text,
        &parsed.class_symbol,
        parsed.creation_args,
        summaries,
        "object-spec.ambiguous",
        format!(
            "{} matches multiple Runtime object candidates: {candidate_list}",
            parsed.class_symbol
        ),
    )
}

fn project_patch_object_kind(patch_id: &str) -> String {
    format!(
        "{PROJECT_PATCH_OBJECT_KIND_PREFIX}{}",
        patch_id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '.') {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>()
    )
}

fn project_patch_ports(patch: &PatchDefinitionCurrent) -> Vec<ObjectSpecPort> {
    skenion_contracts::derive_patch_contract_v01(patch)
        .ports
        .iter()
        .map(|port| object_spec_port_from_current(&port.port))
        .collect()
}

fn object_spec_port_from_current(port: &PortSpecCurrent) -> ObjectSpecPort {
    ObjectSpecPort {
        id: port.id.clone(),
        direction: match &port.direction {
            PortDirectionCurrent::Input => ObjectSpecPortDirection::Input,
            PortDirectionCurrent::Output => ObjectSpecPortDirection::Output,
        },
        port_type: port.port_type.clone(),
        label: port.label.clone(),
        rate: match port.rate.as_ref().unwrap_or(&PortRateCurrent::Control) {
            PortRateCurrent::Event => ObjectSpecPortRate::Event,
            PortRateCurrent::Control => ObjectSpecPortRate::Control,
            PortRateCurrent::Audio => ObjectSpecPortRate::Audio,
            PortRateCurrent::Render => ObjectSpecPortRate::Render,
            PortRateCurrent::Gpu => ObjectSpecPortRate::Gpu,
            PortRateCurrent::Resource => ObjectSpecPortRate::Resource,
            PortRateCurrent::Io => ObjectSpecPortRate::Io,
        },
        accepts: port.accepts.clone(),
        activation: port.trigger_mode.as_ref().map(|mode| match mode {
            skenion_contracts::TriggerModeV01::Trigger => ObjectSpecPortActivation::Trigger,
            skenion_contracts::TriggerModeV01::Latched => ObjectSpecPortActivation::Latched,
            skenion_contracts::TriggerModeV01::Passive => ObjectSpecPortActivation::Passive,
        }),
        message_keys: port.message_keys.clone(),
    }
}

pub(crate) fn is_payload_identity_kind(kind: &str) -> bool {
    matches!(
        kind,
        "value"
            | "data"
            | "payload"
            | "bool"
            | "string"
            | "object.core.bool"
            | "object.core.string"
            | "value.core.message"
            | "value.core.bang"
            | "value.core.string"
            | "value.core.tensor"
    ) || kind.starts_with("value.")
        || kind.starts_with("data.")
        || kind.starts_with("payload.")
        || kind.starts_with("control.")
}

#[cfg(test)]
use ports::input_port;
use ports::{
    audio_binary_ports, audio_input_ports, audio_osc_ports, audio_output_ports, audio_sig_ports,
    bang_ports, comment_ports, control_operator_ports, control_sqrt_ports, message_ports,
    stored_value_ports,
};
use projection::object_spec_port_to_current;
pub(crate) use projection::{
    materialize_object_spec_node_v01, materialize_unresolved_object_spec_node_v01,
    object_spec_node_definition_v01, unresolved_object_spec_node_definition_v01,
};

fn resolve_control_operator(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let kind = candidate.kind.as_str();
    if kind == "object.core.operator.sqrt" {
        if !creation_args.is_empty() {
            return failure(
                input,
                display_text,
                class_symbol,
                creation_args,
                "object-spec.invalid-arg-count",
                "sqrt accepts no creation arguments",
            );
        }
        return success(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            Map::new(),
            control_sqrt_ports(),
        );
    }

    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }

    let right = match creation_args.first() {
        Some(arg) => match numeric_value(arg) {
            Some(value) => value,
            None => {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-spec.invalid-arg-type",
                    format!("{class_symbol} creation argument must be numeric"),
                );
            }
        },
        None => 0.0,
    };
    let mut params = Map::new();
    insert_number(&mut params, "right", right);
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        candidate,
        params,
        control_operator_ports(),
    )
}

fn resolve_control_value(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let kind = candidate.kind.as_str();
    match kind {
        "object.core.bang" => {
            if !creation_args.is_empty() {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-spec.invalid-arg-count",
                    format!("{class_symbol} accepts no creation arguments"),
                );
            }
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                candidate,
                Map::new(),
                bang_ports(),
            )
        }
        "object.core.message" | "object.core.comment" => {
            let text = creation_args
                .iter()
                .map(atom_display_text)
                .collect::<Vec<_>>()
                .join(" ");
            let mut params = Map::new();
            params.insert("text".to_owned(), Value::String(text));
            let ports = if kind == "object.core.message" {
                message_ports()
            } else {
                comment_ports()
            };
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                candidate,
                params,
                ports,
            )
        }
        "object.core.float" => resolve_number_value(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            NumberValueSpec {
                port_type: "value.core.float32",
                coerce: numeric_value,
                to_json: |value| json!(value),
            },
        ),
        "object.core.int" => resolve_number_value(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            NumberValueSpec {
                port_type: "value.core.int32",
                coerce: integer_value,
                to_json: |value| json!(value),
            },
        ),
        "object.core.uint" => resolve_number_value(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            NumberValueSpec {
                port_type: "value.core.uint32",
                coerce: unsigned_value,
                to_json: |value| json!(value),
            },
        ),
        _ => unreachable!("control value resolver received unknown kind"),
    }
}

struct NumberValueSpec<T> {
    port_type: &'static str,
    coerce: fn(&ObjectSpecAtom) -> Option<T>,
    to_json: fn(T) -> Value,
}

fn resolve_number_value<T>(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    spec: NumberValueSpec<T>,
) -> ObjectSpecResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }

    let value = match creation_args.first() {
        Some(arg) => match (spec.coerce)(arg) {
            Some(value) => (spec.to_json)(value),
            None => {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-spec.invalid-arg-type",
                    format!("{class_symbol} creation argument has the wrong numeric type"),
                );
            }
        },
        None => json!(0),
    };
    let mut params = Map::new();
    params.insert("value".to_owned(), value);
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        candidate,
        params,
        stored_value_ports(spec.port_type),
    )
}

fn resolve_audio_object(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let kind = candidate.kind.as_str();
    match kind {
        "object.core.audio.sig" => resolve_audio_number_param(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            AudioNumberParamSpec {
                param_key: "value",
                default_value: 0.0,
                ports: audio_sig_ports(),
            },
        ),
        "object.core.audio.osc" => resolve_audio_number_param(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            AudioNumberParamSpec {
                param_key: "frequency",
                default_value: 440.0,
                ports: audio_osc_ports(),
            },
        ),
        "object.core.audio.operator.mul" => {
            if !creation_args.is_empty() {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-spec.invalid-arg-count",
                    "*~ accepts no creation arguments in the current Runtime audio substrate",
                );
            }
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                candidate,
                Map::new(),
                audio_binary_ports(),
            )
        }
        "object.core.audio.input" | "object.core.audio.output" => {
            if !creation_args.is_empty() {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-spec.invalid-arg-count",
                    format!("{class_symbol} accepts no creation arguments"),
                );
            }
            let ports = if kind == "object.core.audio.input" {
                audio_input_ports()
            } else {
                audio_output_ports()
            };
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                candidate,
                Map::new(),
                ports,
            )
        }
        _ => unreachable!("audio object resolver received unknown kind"),
    }
}

struct AudioNumberParamSpec {
    param_key: &'static str,
    default_value: f64,
    ports: Vec<ObjectSpecPort>,
}

fn resolve_audio_number_param(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    spec: AudioNumberParamSpec,
) -> ObjectSpecResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }
    let value = match creation_args.first() {
        Some(arg) => match numeric_value(arg) {
            Some(value) => value,
            None => {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-spec.invalid-arg-type",
                    format!("{class_symbol} creation argument must be numeric"),
                );
            }
        },
        None => spec.default_value,
    };
    let mut params = Map::new();
    insert_number(&mut params, spec.param_key, value);
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        candidate,
        params,
        spec.ports,
    )
}

fn resolve_named_ref_object(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    param_key: &'static str,
    count_message: &'static str,
) -> ObjectSpecResolution {
    if creation_args.len() != 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-count",
            count_message,
        );
    }
    let Some(reference) = symbol_value(&creation_args[0]) else {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-type",
            format!("{class_symbol} reference argument must be a symbol"),
        );
    };
    let mut params = Map::new();
    params.insert(param_key.to_owned(), Value::String(reference));
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        candidate,
        params,
        Vec::new(),
    )
}

fn resolve_optional_named_ref_object(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    param_key: &'static str,
) -> ObjectSpecResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }
    let mut params = Map::new();
    if let Some(arg) = creation_args.first() {
        let Some(reference) = symbol_value(arg) else {
            return failure(
                input,
                display_text,
                class_symbol,
                creation_args,
                "object-spec.invalid-arg-type",
                format!("{class_symbol} reference argument must be a symbol"),
            );
        };
        params.insert(param_key.to_owned(), Value::String(reference));
    }
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        candidate,
        params,
        Vec::new(),
    )
}

fn success(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    params: Map<String, Value>,
    instance_ports: Vec<ObjectSpecPort>,
) -> ObjectSpecResolution {
    let summary = candidate.summary();
    ObjectSpecResolution {
        input: input.to_owned(),
        display_text,
        class_symbol: class_symbol.to_owned(),
        creation_args,
        resolved_kind: Some(candidate.kind.clone()),
        resolved_kind_version: Some(candidate.kind_version.clone()),
        params,
        instance_ports,
        candidates: vec![summary],
        diagnostics: Vec::new(),
    }
}

fn failure(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ObjectSpecResolution {
    failure_with_candidates(
        input,
        display_text,
        class_symbol,
        creation_args,
        Vec::new(),
        code,
        message,
    )
}

fn failure_with_candidates(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidates: Vec<ObjectSpecCandidateSummary>,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ObjectSpecResolution {
    ObjectSpecResolution {
        input: input.to_owned(),
        display_text,
        class_symbol: class_symbol.to_owned(),
        creation_args,
        resolved_kind: None,
        resolved_kind_version: None,
        params: Map::new(),
        instance_ports: Vec::new(),
        candidates,
        diagnostics: vec![ObjectSpecDiagnostic {
            code: code.into(),
            message: message.into(),
        }],
    }
}

fn numeric_value(atom: &ObjectSpecAtom) -> Option<f64> {
    match atom {
        ObjectSpecAtom::Float(value) => Some(*value),
        ObjectSpecAtom::Int(value) => Some(*value as f64),
        ObjectSpecAtom::Bool(_) | ObjectSpecAtom::Symbol(_) => None,
    }
}

fn integer_value(atom: &ObjectSpecAtom) -> Option<i64> {
    match atom {
        ObjectSpecAtom::Int(value) => Some(*value),
        ObjectSpecAtom::Float(_) | ObjectSpecAtom::Bool(_) | ObjectSpecAtom::Symbol(_) => None,
    }
}

fn unsigned_value(atom: &ObjectSpecAtom) -> Option<u64> {
    match atom {
        ObjectSpecAtom::Int(value) if *value >= 0 => Some(*value as u64),
        ObjectSpecAtom::Float(_) | ObjectSpecAtom::Bool(_) | ObjectSpecAtom::Symbol(_) => None,
        ObjectSpecAtom::Int(_) => None,
    }
}

fn symbol_value(atom: &ObjectSpecAtom) -> Option<String> {
    match atom {
        ObjectSpecAtom::Symbol(value) if !value.is_empty() => Some(value.clone()),
        ObjectSpecAtom::Float(_) | ObjectSpecAtom::Int(_) | ObjectSpecAtom::Bool(_) => None,
        ObjectSpecAtom::Symbol(_) => None,
    }
}

fn atom_display_text(atom: &ObjectSpecAtom) -> String {
    match atom {
        ObjectSpecAtom::Float(value) => value.to_string(),
        ObjectSpecAtom::Int(value) => value.to_string(),
        ObjectSpecAtom::Bool(value) => value.to_string(),
        ObjectSpecAtom::Symbol(value) => value.clone(),
    }
}

fn insert_number(params: &mut Map<String, Value>, key: &str, value: f64) {
    params.insert(key.to_owned(), json!(value));
}

fn unsupported_first_party_audio_message(class_symbol: &str) -> Option<&'static str> {
    match class_symbol {
        "+~"
        | "-~"
        | "/~"
        | "object.core.audio.operator.add"
        | "object.core.audio.operator.sub"
        | "object.core.audio.operator.div" => {
            Some("audio add/sub/div aliases are not executable in the current Runtime substrate")
        }
        "sqrt~" | "object.core.audio.operator.sqrt" => {
            Some("audio sqrt is not executable in the current Runtime substrate")
        }
        "phasor~" | "object.core.audio.phasor" => {
            Some("audio phasor is not executable in the current Runtime substrate")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
