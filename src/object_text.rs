use serde_json::{Map, Value, json};
use skenion_contracts::{
    MessageKeyPolicyV01, NodeCatalogDiagnosticNodeDefinitionReasonV01,
    NodeCatalogDiagnosticNodeDefinitionV01, NodeCatalogDisplayPaletteV01, NodeCatalogDisplayV01,
    NodeCatalogEntryV01, NodeCatalogSnapshotV01, NodeCatalogSourceV01, PackageChecksumAlgorithmV01,
    PackageChecksumV01,
};

use crate::{
    GraphNodeCurrent, NodeDefinitionCurrent, PatchDefinitionCurrent, PortDirectionCurrent,
    PortRateCurrent, PortSpecCurrent, ProjectDocumentCurrent,
    nodes::{CoreNodeConstructor, CoreNodeImplementation, first_party_core_nodes},
};

const CURRENT_KIND_VERSION: &str = "0.1.0";
pub(crate) const PROJECT_PATCH_OBJECT_KIND_PREFIX: &str = "object.project.patch.";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ObjectTextResolution {
    pub(crate) input: String,
    pub(crate) display_text: String,
    pub(crate) class_symbol: String,
    pub(crate) creation_args: Vec<ObjectTextAtom>,
    pub(crate) resolved_kind: Option<String>,
    pub(crate) resolved_kind_version: Option<String>,
    pub(crate) params: Map<String, Value>,
    pub(crate) instance_ports: Vec<ObjectTextPort>,
    pub(crate) candidates: Vec<ObjectTextCandidateSummary>,
    pub(crate) diagnostics: Vec<ObjectTextDiagnostic>,
}

impl ObjectTextResolution {
    pub(crate) fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectTextCandidateSummary {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) kind: String,
    pub(crate) display_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ObjectTextAtom {
    Float(f64),
    Int(i64),
    Bool(bool),
    Symbol(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectTextDiagnostic {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectTextPort {
    pub(crate) id: String,
    pub(crate) direction: ObjectTextPortDirection,
    pub(crate) port_type: String,
    pub(crate) label: Option<String>,
    pub(crate) rate: ObjectTextPortRate,
    pub(crate) accepts: Option<Vec<String>>,
    pub(crate) activation: Option<ObjectTextPortActivation>,
    pub(crate) message_keys: Option<MessageKeyPolicyV01>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectTextPortDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectTextPortRate {
    Event,
    Control,
    Audio,
    Render,
    Gpu,
    Resource,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectTextPortActivation {
    Trigger,
    Latched,
    Passive,
}

#[derive(Debug, Clone)]
pub(crate) struct ObjectRegistry {
    candidates: Vec<ObjectRegistryCandidate>,
    allow_unchecked_project_patch_refs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
enum ObjectRegistrySource {
    FirstPartyCore,
    ProjectPatch,
    PackageProvider,
    NativeProvider,
}

#[derive(Debug, Clone)]
struct ObjectRegistryCandidate {
    id: String,
    source: ObjectRegistrySource,
    aliases: Vec<String>,
    kind: String,
    kind_version: String,
    display_name: String,
    constructor: Option<CoreNodeConstructor>,
    catalog_category: Option<&'static str>,
    project_patch: Option<ProjectPatchCandidate>,
}

#[derive(Debug, Clone)]
struct ProjectPatchCandidate {
    patch_id: String,
    revision: String,
    description: Option<String>,
    interface_digest: PackageChecksumV01,
    ports: Vec<ObjectTextPort>,
}

#[derive(Debug, Clone)]
struct ParsedObjectText {
    input: String,
    display_text: String,
    class_symbol: String,
    creation_args: Vec<ObjectTextAtom>,
}

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

    pub(crate) fn resolve(&self, input: &str) -> ObjectTextResolution {
        let parsed = match parse_object_text_input_v01(input) {
            Ok(parsed) => parsed,
            Err(resolution) => return *resolution,
        };

        if is_payload_identity_kind(&parsed.class_symbol) {
            return failure(
                &parsed.input,
                parsed.display_text,
                &parsed.class_symbol,
                parsed.creation_args,
                "object-text.payload-identity",
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
                "object-text.unsupported-first-party",
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
                definition: unresolved_object_text_node_definition_v01(),
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

        let canonical_object_text = candidate.canonical_object_text()?;
        let resolution = self.resolve(&canonical_object_text);
        if !resolution.ok() {
            return None;
        }
        let mut definition = object_text_node_definition_v01(&resolution)?;
        definition.display_name = candidate.display_name.clone();
        definition.category = core_catalog_category(candidate).to_owned();

        Some(NodeCatalogEntryV01 {
            catalog_id: catalog_id_for_core_candidate(candidate),
            canonical_object_text,
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

    fn lookup_candidates(&self, parsed: &ParsedObjectText) -> Vec<ObjectRegistryCandidate> {
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
        parsed: &ParsedObjectText,
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
        parsed: ParsedObjectText,
        candidate: &ObjectRegistryCandidate,
    ) -> ObjectTextResolution {
        match candidate.source {
            ObjectRegistrySource::FirstPartyCore => construct_first_party_core(parsed, candidate),
            ObjectRegistrySource::ProjectPatch => construct_project_patch(parsed, candidate),
            ObjectRegistrySource::PackageProvider | ObjectRegistrySource::NativeProvider => {
                failure(
                    &parsed.input,
                    parsed.display_text,
                    &parsed.class_symbol,
                    parsed.creation_args,
                    "object-text.provider-unavailable",
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

    fn summary(&self) -> ObjectTextCandidateSummary {
        ObjectTextCandidateSummary {
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

    fn canonical_object_text(&self) -> Option<String> {
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
        canonical_object_text: patch.patch_id.clone(),
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
        .map(object_text_port_to_current)
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

pub(crate) fn resolve_object_text_v01(input: &str) -> ObjectTextResolution {
    ObjectRegistry::first_party_core()
        .allow_unchecked_project_patch_refs()
        .resolve(input)
}

fn parse_object_text_input_v01(input: &str) -> Result<ParsedObjectText, Box<ObjectTextResolution>> {
    let parsed = skenion_contracts::parse_object_text_v01(input);
    let creation_args = parsed
        .creation_args
        .iter()
        .map(contract_object_text_atom_to_runtime)
        .collect::<Vec<_>>();
    if parsed.ok {
        return Ok(ParsedObjectText {
            input: parsed.input,
            display_text: parsed.display_text,
            class_symbol: parsed.class_name,
            creation_args,
        });
    }

    let diagnostic = parsed.diagnostics.first();
    let code = diagnostic
        .map(|diagnostic| runtime_object_text_diagnostic_code(&diagnostic.code))
        .unwrap_or_else(|| "object-text.invalid-syntax".to_owned());
    let message = diagnostic
        .map(|diagnostic| diagnostic.message.clone())
        .unwrap_or_else(|| "object text could not be parsed".to_owned());
    Err(Box::new(failure(
        &parsed.input,
        parsed.display_text,
        &parsed.class_name,
        creation_args,
        code,
        message,
    )))
}

fn runtime_object_text_diagnostic_code(code: &str) -> String {
    match code {
        "empty-object-text" => "object-text.empty".to_owned(),
        "invalid-syntax" => "object-text.invalid-syntax".to_owned(),
        value if value.starts_with("object-text.") => value.to_owned(),
        value => format!("object-text.{value}"),
    }
}

fn contract_object_text_atom_to_runtime(
    atom: &skenion_contracts::ObjectTextAtomV01,
) -> ObjectTextAtom {
    match atom {
        skenion_contracts::ObjectTextAtomV01::Float { value, .. } => ObjectTextAtom::Float(*value),
        skenion_contracts::ObjectTextAtomV01::Int { value, .. } => ObjectTextAtom::Int(*value),
        skenion_contracts::ObjectTextAtomV01::Uint { value, .. } => {
            if *value <= i64::MAX as u64 {
                ObjectTextAtom::Int(*value as i64)
            } else {
                ObjectTextAtom::Symbol(value.to_string())
            }
        }
        skenion_contracts::ObjectTextAtomV01::Bool { value } => ObjectTextAtom::Bool(*value),
        skenion_contracts::ObjectTextAtomV01::Identifier { value }
        | skenion_contracts::ObjectTextAtomV01::String { value } => {
            ObjectTextAtom::Symbol(value.clone())
        }
    }
}

fn construct_first_party_core(
    parsed: ParsedObjectText,
    candidate: &ObjectRegistryCandidate,
) -> ObjectTextResolution {
    let ParsedObjectText {
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
                "subpatch object text requires exactly one patch reference",
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
        "object-text.unresolved",
        format!(
            "{} is registered but has no Runtime constructor",
            candidate.kind
        ),
    )
}

fn construct_project_patch(
    parsed: ParsedObjectText,
    candidate: &ObjectRegistryCandidate,
) -> ObjectTextResolution {
    let ParsedObjectText {
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
            "object-text.unresolved",
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
                "object-text.invalid-arg-count",
                "subpatch object text requires exactly one patch reference",
            );
        }
        let Some(reference) = symbol_value(&creation_args[0]) else {
            return failure_with_candidates(
                &input,
                display_text,
                &class_symbol,
                creation_args,
                vec![candidate.summary()],
                "object-text.invalid-arg-type",
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
                "object-text.unresolved",
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
            "object-text.invalid-arg-count",
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

fn explicit_project_patch_ref(parsed: &ParsedObjectText) -> Option<String> {
    if parsed.creation_args.len() != 1 {
        return None;
    }
    symbol_value(&parsed.creation_args[0])
}

fn unresolved_resolution(parsed: ParsedObjectText) -> ObjectTextResolution {
    failure(
        &parsed.input,
        parsed.display_text,
        &parsed.class_symbol,
        parsed.creation_args,
        "object-text.unresolved",
        format!(
            "{} is not available in the local Runtime object registry",
            parsed.class_symbol
        ),
    )
}

fn ambiguous_resolution(
    parsed: ParsedObjectText,
    candidates: Vec<ObjectRegistryCandidate>,
) -> ObjectTextResolution {
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
        "object-text.ambiguous",
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

fn project_patch_ports(patch: &PatchDefinitionCurrent) -> Vec<ObjectTextPort> {
    skenion_contracts::derive_patch_contract_v01(patch)
        .ports
        .iter()
        .map(|port| object_text_port_from_current(&port.port))
        .collect()
}

fn object_text_port_from_current(port: &PortSpecCurrent) -> ObjectTextPort {
    ObjectTextPort {
        id: port.id.clone(),
        direction: match &port.direction {
            PortDirectionCurrent::Input => ObjectTextPortDirection::Input,
            PortDirectionCurrent::Output => ObjectTextPortDirection::Output,
        },
        port_type: port.port_type.clone(),
        label: port.label.clone(),
        rate: match port.rate.as_ref().unwrap_or(&PortRateCurrent::Control) {
            PortRateCurrent::Event => ObjectTextPortRate::Event,
            PortRateCurrent::Control => ObjectTextPortRate::Control,
            PortRateCurrent::Audio => ObjectTextPortRate::Audio,
            PortRateCurrent::Render => ObjectTextPortRate::Render,
            PortRateCurrent::Gpu => ObjectTextPortRate::Gpu,
            PortRateCurrent::Resource => ObjectTextPortRate::Resource,
            PortRateCurrent::Io => ObjectTextPortRate::Io,
        },
        accepts: port.accepts.clone(),
        activation: port.trigger_mode.as_ref().map(|mode| match mode {
            skenion_contracts::TriggerModeV01::Trigger => ObjectTextPortActivation::Trigger,
            skenion_contracts::TriggerModeV01::Latched => ObjectTextPortActivation::Latched,
            skenion_contracts::TriggerModeV01::Passive => ObjectTextPortActivation::Passive,
        }),
        message_keys: port.message_keys.clone(),
    }
}

pub(crate) fn materialize_object_text_node_v01(
    resolution: &ObjectTextResolution,
    node_id: impl Into<String>,
) -> Result<GraphNodeCurrent, ObjectTextDiagnostic> {
    let Some(resolved_kind) = resolution.resolved_kind.clone() else {
        return Err(primary_resolution_diagnostic(resolution));
    };
    let Some(resolved_kind_version) = resolution.resolved_kind_version.clone() else {
        return Err(primary_resolution_diagnostic(resolution));
    };

    Ok(GraphNodeCurrent {
        id: node_id.into(),
        kind: resolved_kind,
        kind_version: resolved_kind_version,
        object_text: Some(resolution.display_text.clone()),
        binding_ref: None,
        params: resolution.params.clone(),
        ports: resolution
            .instance_ports
            .iter()
            .map(object_text_port_to_current)
            .collect(),
        port_groups: None,
    })
}

pub(crate) fn object_text_node_definition_v01(
    resolution: &ObjectTextResolution,
) -> Option<NodeDefinitionCurrent> {
    let resolved_kind = resolution.resolved_kind.as_ref()?;
    let resolved_kind_version = resolution.resolved_kind_version.as_ref()?;
    let ports = resolution
        .instance_ports
        .iter()
        .map(object_text_port_to_current)
        .collect::<Vec<_>>();
    let has_audio_port = ports
        .iter()
        .any(|port| port.rate == Some(PortRateCurrent::Audio));

    Some(NodeDefinitionCurrent {
        schema: "skenion.node.definition".to_owned(),
        schema_version: CURRENT_KIND_VERSION.to_owned(),
        id: resolved_kind.clone(),
        version: resolved_kind_version.clone(),
        display_name: object_text_definition_display_name(resolved_kind),
        category: object_text_definition_category(resolved_kind).to_owned(),
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
    })
}

pub(crate) fn materialize_unresolved_object_text_node_v01(
    resolution: &ObjectTextResolution,
    node_id: impl Into<String>,
) -> GraphNodeCurrent {
    let diagnostic = primary_resolution_diagnostic(resolution);
    let mut params = Map::new();
    params.insert(
        "objectText".to_owned(),
        Value::String(resolution.display_text.clone()),
    );
    params.insert(
        "requestedKind".to_owned(),
        Value::String(resolution.class_symbol.clone()),
    );
    params.insert("diagnosticCode".to_owned(), Value::String(diagnostic.code));
    params.insert(
        "diagnosticMessage".to_owned(),
        Value::String(diagnostic.message),
    );
    params.insert(
        "candidateCount".to_owned(),
        json!(resolution.candidates.len()),
    );
    if !resolution.candidates.is_empty() {
        params.insert(
            "candidates".to_owned(),
            Value::Array(
                resolution
                    .candidates
                    .iter()
                    .map(object_text_candidate_json)
                    .collect(),
            ),
        );
    }

    GraphNodeCurrent {
        id: node_id.into(),
        kind: "object.core.unresolved".to_owned(),
        kind_version: CURRENT_KIND_VERSION.to_owned(),
        object_text: Some(resolution.display_text.clone()),
        binding_ref: None,
        params,
        ports: Vec::new(),
        port_groups: None,
    }
}

fn object_text_candidate_json(candidate: &ObjectTextCandidateSummary) -> Value {
    json!({
        "id": candidate.id,
        "source": candidate.source,
        "kind": candidate.kind,
        "displayName": candidate.display_name,
    })
}

pub(crate) fn unresolved_object_text_node_definition_v01() -> NodeDefinitionCurrent {
    NodeDefinitionCurrent {
        schema: "skenion.node.definition".to_owned(),
        schema_version: CURRENT_KIND_VERSION.to_owned(),
        id: "object.core.unresolved".to_owned(),
        version: CURRENT_KIND_VERSION.to_owned(),
        display_name: "Unresolved Object".to_owned(),
        category: "Diagnostics".to_owned(),
        script_api_version: None,
        bundle_hash: None,
        surface: None,
        ports: Vec::new(),
        port_groups: None,
        execution: skenion_contracts::NodeExecutionV01 {
            model: skenion_contracts::ExecutionModelV01::Event,
            clock: None,
        },
        state: skenion_contracts::NodeStateV01 { persistent: false },
        permissions: Vec::new(),
        capabilities: vec!["diagnostic.unresolved-object.v0.1".to_owned()],
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

fn primary_resolution_diagnostic(resolution: &ObjectTextResolution) -> ObjectTextDiagnostic {
    resolution
        .diagnostics
        .first()
        .cloned()
        .unwrap_or_else(|| ObjectTextDiagnostic {
            code: "object-text.unresolved".to_owned(),
            message: format!(
                "{} is not available in the local Runtime object resolver",
                resolution.class_symbol
            ),
        })
}

fn object_text_port_to_current(port: &ObjectTextPort) -> PortSpecCurrent {
    PortSpecCurrent {
        id: port.id.clone(),
        direction: match &port.direction {
            ObjectTextPortDirection::Input => PortDirectionCurrent::Input,
            ObjectTextPortDirection::Output => PortDirectionCurrent::Output,
        },
        port_type: port.port_type.clone(),
        label: port.label.clone(),
        rate: Some(match &port.rate {
            ObjectTextPortRate::Event => PortRateCurrent::Event,
            ObjectTextPortRate::Control => PortRateCurrent::Control,
            ObjectTextPortRate::Audio => PortRateCurrent::Audio,
            ObjectTextPortRate::Render => PortRateCurrent::Render,
            ObjectTextPortRate::Gpu => PortRateCurrent::Gpu,
            ObjectTextPortRate::Resource => PortRateCurrent::Resource,
            ObjectTextPortRate::Io => PortRateCurrent::Io,
        }),
        accepts: port.accepts.clone().or_else(|| message_input_accepts(port)),
        min_connections: None,
        max_connections: None,
        merge_policy: None,
        fan_out_policy: None,
        trigger_mode: port.activation.as_ref().map(|activation| match activation {
            ObjectTextPortActivation::Trigger => skenion_contracts::TriggerModeV01::Trigger,
            ObjectTextPortActivation::Latched => skenion_contracts::TriggerModeV01::Latched,
            ObjectTextPortActivation::Passive => skenion_contracts::TriggerModeV01::Passive,
        }),
        message_keys: port
            .message_keys
            .clone()
            .or_else(|| default_message_input_key_policy(port)),
        default_value: None,
        latch: None,
        required: matches!(&port.direction, ObjectTextPortDirection::Input).then_some(false),
        style_key: None,
        group: None,
        description: None,
    }
}

fn message_input_accepts(port: &ObjectTextPort) -> Option<Vec<String>> {
    if matches!(&port.direction, ObjectTextPortDirection::Input)
        && port.port_type == "value.core.message"
    {
        return Some(
            [
                "value.core.float32",
                "value.core.int32",
                "value.core.uint32",
                "value.core.bool",
                "value.core.bang",
                "value.core.message",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        );
    }
    None
}

fn default_message_input_key_policy(port: &ObjectTextPort) -> Option<MessageKeyPolicyV01> {
    if matches!(&port.direction, ObjectTextPortDirection::Input)
        && port.port_type == "value.core.message"
    {
        return Some(message_key_policy(
            &["bang", "set", "float", "int", "uint", "bool", "message"],
            &["set"],
            &["bang", "float", "int", "uint", "bool", "message"],
            &["set", "float", "int", "uint", "bool", "message"],
            &["bang", "float", "int", "uint", "bool", "message"],
        ));
    }
    None
}

fn object_text_definition_display_name(kind: &str) -> String {
    kind.rsplit('.')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(kind)
        .replace('-', " ")
}

fn object_text_definition_category(kind: &str) -> &'static str {
    if kind.starts_with("object.core.audio.") {
        "Runtime Audio"
    } else {
        "Runtime Objects"
    }
}

fn resolve_control_operator(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    candidate: &ObjectRegistryCandidate,
) -> ObjectTextResolution {
    let kind = candidate.kind.as_str();
    if kind == "object.core.operator.sqrt" {
        if !creation_args.is_empty() {
            return failure(
                input,
                display_text,
                class_symbol,
                creation_args,
                "object-text.invalid-arg-count",
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
            "object-text.invalid-arg-count",
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
                    "object-text.invalid-arg-type",
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
    creation_args: Vec<ObjectTextAtom>,
    candidate: &ObjectRegistryCandidate,
) -> ObjectTextResolution {
    let kind = candidate.kind.as_str();
    match kind {
        "object.core.bang" => {
            if !creation_args.is_empty() {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-text.invalid-arg-count",
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
    coerce: fn(&ObjectTextAtom) -> Option<T>,
    to_json: fn(T) -> Value,
}

fn resolve_number_value<T>(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    candidate: &ObjectRegistryCandidate,
    spec: NumberValueSpec<T>,
) -> ObjectTextResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-count",
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
                    "object-text.invalid-arg-type",
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
    creation_args: Vec<ObjectTextAtom>,
    candidate: &ObjectRegistryCandidate,
) -> ObjectTextResolution {
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
                    "object-text.invalid-arg-count",
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
                    "object-text.invalid-arg-count",
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
    ports: Vec<ObjectTextPort>,
}

fn resolve_audio_number_param(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    candidate: &ObjectRegistryCandidate,
    spec: AudioNumberParamSpec,
) -> ObjectTextResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-count",
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
                    "object-text.invalid-arg-type",
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
    creation_args: Vec<ObjectTextAtom>,
    candidate: &ObjectRegistryCandidate,
    param_key: &'static str,
    count_message: &'static str,
) -> ObjectTextResolution {
    if creation_args.len() != 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-count",
            count_message,
        );
    }
    let Some(reference) = symbol_value(&creation_args[0]) else {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-type",
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
    creation_args: Vec<ObjectTextAtom>,
    candidate: &ObjectRegistryCandidate,
    param_key: &'static str,
) -> ObjectTextResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-count",
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
                "object-text.invalid-arg-type",
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
    creation_args: Vec<ObjectTextAtom>,
    candidate: &ObjectRegistryCandidate,
    params: Map<String, Value>,
    instance_ports: Vec<ObjectTextPort>,
) -> ObjectTextResolution {
    let summary = candidate.summary();
    ObjectTextResolution {
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
    creation_args: Vec<ObjectTextAtom>,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ObjectTextResolution {
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
    creation_args: Vec<ObjectTextAtom>,
    candidates: Vec<ObjectTextCandidateSummary>,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ObjectTextResolution {
    ObjectTextResolution {
        input: input.to_owned(),
        display_text,
        class_symbol: class_symbol.to_owned(),
        creation_args,
        resolved_kind: None,
        resolved_kind_version: None,
        params: Map::new(),
        instance_ports: Vec::new(),
        candidates,
        diagnostics: vec![ObjectTextDiagnostic {
            code: code.into(),
            message: message.into(),
        }],
    }
}

fn numeric_value(atom: &ObjectTextAtom) -> Option<f64> {
    match atom {
        ObjectTextAtom::Float(value) => Some(*value),
        ObjectTextAtom::Int(value) => Some(*value as f64),
        ObjectTextAtom::Bool(_) | ObjectTextAtom::Symbol(_) => None,
    }
}

fn integer_value(atom: &ObjectTextAtom) -> Option<i64> {
    match atom {
        ObjectTextAtom::Int(value) => Some(*value),
        ObjectTextAtom::Float(_) | ObjectTextAtom::Bool(_) | ObjectTextAtom::Symbol(_) => None,
    }
}

fn unsigned_value(atom: &ObjectTextAtom) -> Option<u64> {
    match atom {
        ObjectTextAtom::Int(value) if *value >= 0 => Some(*value as u64),
        ObjectTextAtom::Float(_) | ObjectTextAtom::Bool(_) | ObjectTextAtom::Symbol(_) => None,
        ObjectTextAtom::Int(_) => None,
    }
}

fn symbol_value(atom: &ObjectTextAtom) -> Option<String> {
    match atom {
        ObjectTextAtom::Symbol(value) if !value.is_empty() => Some(value.clone()),
        ObjectTextAtom::Float(_) | ObjectTextAtom::Int(_) | ObjectTextAtom::Bool(_) => None,
        ObjectTextAtom::Symbol(_) => None,
    }
}

fn atom_display_text(atom: &ObjectTextAtom) -> String {
    match atom {
        ObjectTextAtom::Float(value) => value.to_string(),
        ObjectTextAtom::Int(value) => value.to_string(),
        ObjectTextAtom::Bool(value) => value.to_string(),
        ObjectTextAtom::Symbol(value) => value.clone(),
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

fn input_port(
    id: &str,
    port_type: &str,
    rate: ObjectTextPortRate,
    activation: ObjectTextPortActivation,
) -> ObjectTextPort {
    ObjectTextPort {
        id: id.to_owned(),
        direction: ObjectTextPortDirection::Input,
        port_type: port_type.to_owned(),
        label: None,
        rate,
        accepts: None,
        activation: Some(activation),
        message_keys: None,
    }
}

fn output_port(id: &str, port_type: &str, rate: ObjectTextPortRate) -> ObjectTextPort {
    ObjectTextPort {
        id: id.to_owned(),
        direction: ObjectTextPortDirection::Output,
        port_type: port_type.to_owned(),
        label: None,
        rate,
        accepts: None,
        activation: None,
        message_keys: None,
    }
}

fn with_accepts(mut port: ObjectTextPort, accepts: &[&str]) -> ObjectTextPort {
    port.accepts = Some(string_list(accepts));
    port
}

fn with_message_keys(mut port: ObjectTextPort, policy: MessageKeyPolicyV01) -> ObjectTextPort {
    port.message_keys = Some(policy);
    port
}

fn message_input_port(
    id: &str,
    activation: ObjectTextPortActivation,
    accepts: &[&str],
    policy: MessageKeyPolicyV01,
) -> ObjectTextPort {
    with_message_keys(
        with_accepts(
            input_port(
                id,
                "value.core.message",
                ObjectTextPortRate::Control,
                activation,
            ),
            accepts,
        ),
        policy,
    )
}

fn message_key_policy(
    accepted: &[&str],
    silent: &[&str],
    trigger: &[&str],
    store: &[&str],
    emit: &[&str],
) -> MessageKeyPolicyV01 {
    MessageKeyPolicyV01 {
        accepted: string_list(accepted),
        silent: optional_string_list(silent),
        trigger: optional_string_list(trigger),
        store: optional_string_list(store),
        emit: optional_string_list(emit),
    }
}

fn string_list(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn optional_string_list(values: &[&str]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| string_list(values))
}

fn numeric_message_input_policy() -> MessageKeyPolicyV01 {
    message_key_policy(
        &["bang", "set", "float", "int", "uint", "bool"],
        &["set"],
        &["bang", "float", "int", "uint", "bool"],
        &["set", "float", "int", "uint", "bool"],
        &["bang", "float", "int", "uint", "bool"],
    )
}

fn bang_message_input_policy() -> MessageKeyPolicyV01 {
    message_key_policy(&["bang"], &[], &["bang"], &[], &["bang"])
}

fn stored_message_input_policy() -> MessageKeyPolicyV01 {
    message_key_policy(
        &["bang", "set", "float", "int", "uint", "bool", "message"],
        &["set"],
        &["bang", "float", "int", "uint", "bool", "message"],
        &["set", "float", "int", "uint", "bool", "message"],
        &["bang", "float", "int", "uint", "bool", "message"],
    )
}

fn comment_message_input_policy() -> MessageKeyPolicyV01 {
    message_key_policy(
        &["set", "float", "int", "uint", "bool", "message"],
        &["set"],
        &["float", "int", "uint", "bool", "message"],
        &["set", "float", "int", "uint", "bool", "message"],
        &[],
    )
}

fn stored_value_ports(port_type: &str) -> Vec<ObjectTextPort> {
    vec![
        message_input_port(
            "in",
            ObjectTextPortActivation::Trigger,
            &[
                "value.core.float32",
                "value.core.int32",
                "value.core.uint32",
                "value.core.bool",
                "value.core.bang",
            ],
            numeric_message_input_policy(),
        ),
        input_port(
            "cold",
            port_type,
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Latched,
        ),
        output_port("value", port_type, ObjectTextPortRate::Control),
    ]
}

fn control_operator_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "in",
            "value.core.float32",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Trigger,
        ),
        input_port(
            "right",
            "value.core.float32",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectTextPortRate::Control),
    ]
}

fn control_sqrt_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "in",
            "value.core.float32",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Trigger,
        ),
        output_port("out", "value.core.float32", ObjectTextPortRate::Control),
    ]
}

fn bang_ports() -> Vec<ObjectTextPort> {
    vec![
        message_input_port(
            "in",
            ObjectTextPortActivation::Trigger,
            &["value.core.bang"],
            bang_message_input_policy(),
        ),
        output_port("out", "value.core.bang", ObjectTextPortRate::Event),
    ]
}

fn message_ports() -> Vec<ObjectTextPort> {
    vec![
        message_input_port(
            "in",
            ObjectTextPortActivation::Trigger,
            &[
                "value.core.float32",
                "value.core.int32",
                "value.core.uint32",
                "value.core.bool",
                "value.core.bang",
                "value.core.message",
            ],
            stored_message_input_policy(),
        ),
        output_port("out", "value.core.message", ObjectTextPortRate::Control),
    ]
}

fn comment_ports() -> Vec<ObjectTextPort> {
    vec![message_input_port(
        "in",
        ObjectTextPortActivation::Trigger,
        &[
            "value.core.float32",
            "value.core.int32",
            "value.core.uint32",
            "value.core.bool",
            "value.core.message",
        ],
        comment_message_input_policy(),
    )]
}

fn audio_sig_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "value",
            "value.core.float32",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectTextPortRate::Audio),
    ]
}

fn audio_osc_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "frequency",
            "value.core.float32",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectTextPortRate::Audio),
    ]
}

fn audio_binary_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "left",
            "value.core.float32",
            ObjectTextPortRate::Audio,
            ObjectTextPortActivation::Latched,
        ),
        input_port(
            "right",
            "value.core.float32",
            ObjectTextPortRate::Audio,
            ObjectTextPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectTextPortRate::Audio),
    ]
}

fn audio_input_ports() -> Vec<ObjectTextPort> {
    vec![
        output_port("left", "value.core.float32", ObjectTextPortRate::Audio),
        output_port("right", "value.core.float32", ObjectTextPortRate::Audio),
    ]
}

fn audio_output_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "left",
            "value.core.float32",
            ObjectTextPortRate::Audio,
            ObjectTextPortActivation::Latched,
        ),
        input_port(
            "right",
            "value.core.float32",
            ObjectTextPortRate::Audio,
            ObjectTextPortActivation::Latched,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_kind(resolution: &ObjectTextResolution, kind: &str) {
        assert!(resolution.ok(), "{resolution:?}");
        assert_eq!(resolution.resolved_kind.as_deref(), Some(kind));
        assert_eq!(resolution.resolved_kind_version.as_deref(), Some("0.1.0"));
    }

    fn assert_diagnostic(resolution: &ObjectTextResolution, code: &str) {
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
        let add = resolve_object_text_v01("[+ 1e3]");
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

        let sqrt = resolve_object_text_v01("sqrt 2");
        assert_eq!(sqrt.diagnostics[0].code, "object-text.invalid-arg-count");

        let invalid = resolve_object_text_v01("+ true");
        assert_eq!(invalid.diagnostics[0].code, "object-text.invalid-arg-type");

        for (input, kind, param, value) in [
            ("- -2", "object.core.operator.sub", "right", json!(-2.0)),
            ("/ 4", "object.core.operator.div", "right", json!(4.0)),
            ("* 3", "object.core.operator.mul", "right", json!(3.0)),
            ("pow 2", "object.core.operator.pow", "right", json!(2.0)),
            ("max 8", "object.core.operator.max", "right", json!(8.0)),
            ("min 1", "object.core.operator.min", "right", json!(1.0)),
        ] {
            let resolution = resolve_object_text_v01(input);
            assert_kind(&resolution, kind);
            assert_eq!(resolution.params[param], value);
            assert_eq!(resolution.instance_ports.len(), 3);
        }

        let sqrt = resolve_object_text_v01("sqrt");
        assert_kind(&sqrt, "object.core.operator.sqrt");
        assert_eq!(sqrt.instance_ports.len(), 2);

        let default_add = resolve_object_text_v01("object.core.operator.add");
        assert_kind(&default_add, "object.core.operator.add");
        assert_eq!(default_add.params["right"], json!(0.0));

        assert_diagnostic(
            &resolve_object_text_v01("sqrt 1"),
            "object-text.invalid-arg-count",
        );
        assert_diagnostic(
            &resolve_object_text_v01("+ 1 2"),
            "object-text.invalid-arg-count",
        );
        assert_diagnostic(
            &resolve_object_text_v01("object.core.operator.mul false"),
            "object-text.invalid-arg-type",
        );
    }

    #[test]
    fn resolves_runtime_value_audio_and_subpatch_aliases() {
        let float = resolve_object_text_v01("f 0.25");
        assert!(float.ok());
        assert_eq!(float.resolved_kind.as_deref(), Some("object.core.float"));
        assert_eq!(float.params["value"], json!(0.25));

        let osc = resolve_object_text_v01("osc~ 220");
        assert!(osc.ok());
        assert_eq!(osc.resolved_kind.as_deref(), Some("object.core.audio.osc"));
        assert_eq!(osc.params["frequency"], json!(220.0));

        let mul = resolve_object_text_v01("*~");
        assert!(mul.ok());
        assert_eq!(
            mul.resolved_kind.as_deref(),
            Some("object.core.audio.operator.mul")
        );
        assert_eq!(mul.instance_ports.len(), 3);

        let scalar_mul = resolve_object_text_v01("*~ 0.5");
        assert_eq!(
            scalar_mul.diagnostics[0].code,
            "object-text.invalid-arg-count"
        );

        let unsupported = resolve_object_text_v01("+~");
        assert_eq!(
            unsupported.diagnostics[0].code,
            "object-text.unsupported-first-party"
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
                &resolve_object_text_v01(input),
                "object-text.unsupported-first-party",
            );
        }

        let sig = resolve_object_text_v01("sig~");
        assert_kind(&sig, "object.core.audio.sig");
        assert_eq!(sig.params["value"], json!(0.0));

        let invalid_sig = resolve_object_text_v01("sig~ false");
        assert_diagnostic(&invalid_sig, "object-text.invalid-arg-type");
        assert_diagnostic(
            &resolve_object_text_v01("sig~ 1 2"),
            "object-text.invalid-arg-count",
        );

        let osc = resolve_object_text_v01("object.core.audio.osc 220");
        assert_kind(&osc, "object.core.audio.osc");
        assert_eq!(osc.params["frequency"], json!(220.0));
        assert_diagnostic(
            &resolve_object_text_v01("osc~ nope"),
            "object-text.invalid-arg-type",
        );

        let audio_input = resolve_object_text_v01("adc~");
        assert_kind(&audio_input, "object.core.audio.input");
        assert_eq!(audio_input.instance_ports[0].id, "left");

        let audio_output = resolve_object_text_v01("dac~");
        assert_kind(&audio_output, "object.core.audio.output");
        assert_eq!(audio_output.instance_ports[0].id, "left");

        let invalid_audio_output = resolve_object_text_v01("dac~ 1");
        assert_diagnostic(&invalid_audio_output, "object-text.invalid-arg-count");

        let subpatch = resolve_object_text_v01("p voice");
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
            let resolution = resolve_object_text_v01(input);
            assert_kind(&resolution, kind);
            assert_eq!(resolution.params["value"], value);
            assert_eq!(resolution.instance_ports.len(), 3);
        }

        assert_diagnostic(
            &resolve_object_text_v01("int 1.5"),
            "object-text.invalid-arg-type",
        );
        assert_diagnostic(
            &resolve_object_text_v01("uint -1"),
            "object-text.invalid-arg-type",
        );
        assert_diagnostic(
            &resolve_object_text_v01("float 1 2"),
            "object-text.invalid-arg-count",
        );

        let bang = resolve_object_text_v01("bang");
        assert_kind(&bang, "object.core.bang");
        assert!(bang.params.is_empty());
        assert_eq!(bang.instance_ports[1].port_type, "value.core.bang");
        assert_diagnostic(
            &resolve_object_text_v01("object.core.bang 1"),
            "object-text.invalid-arg-count",
        );

        let float_alias = resolve_object_text_v01("f 1.5");
        assert_kind(&float_alias, "object.core.float");
        assert_eq!(float_alias.params["value"], json!(1.5));
        assert_diagnostic(
            &resolve_object_text_v01("float true"),
            "object-text.invalid-arg-type",
        );

        let message = resolve_object_text_v01("message set gain");
        assert_kind(&message, "object.core.message");
        assert_eq!(message.params["text"], json!("set gain"));
        let empty_message = resolve_object_text_v01("msg");
        assert_kind(&empty_message, "object.core.message");
        assert_eq!(empty_message.params["text"], json!(""));

        let comment = resolve_object_text_v01("comment hello world");
        assert_kind(&comment, "object.core.comment");
        assert_eq!(comment.params["text"], json!("hello world"));
        assert_eq!(comment.instance_ports.len(), 1);
        let empty_comment = resolve_object_text_v01("object.core.comment");
        assert_kind(&empty_comment, "object.core.comment");
        assert_eq!(empty_comment.params["text"], json!(""));

        let inlet = resolve_object_text_v01("inlet left");
        assert_kind(&inlet, "object.core.inlet");
        assert_eq!(inlet.params["portId"], json!("left"));

        let anonymous_outlet = resolve_object_text_v01("outlet");
        assert_kind(&anonymous_outlet, "object.core.outlet");
        assert!(anonymous_outlet.params.is_empty());
        let named_outlet = resolve_object_text_v01("object.core.outlet right");
        assert_kind(&named_outlet, "object.core.outlet");
        assert_eq!(named_outlet.params["portId"], json!("right"));

        assert_diagnostic(
            &resolve_object_text_v01("p"),
            "object-text.invalid-arg-count",
        );
        assert_diagnostic(
            &resolve_object_text_v01("p true"),
            "object-text.invalid-arg-type",
        );
        assert_diagnostic(
            &resolve_object_text_v01("inlet left right"),
            "object-text.invalid-arg-count",
        );
        assert_diagnostic(
            &resolve_object_text_v01("outlet 1"),
            "object-text.invalid-arg-type",
        );
    }

    #[test]
    fn rejects_payload_identities_as_object_text() {
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
            let resolution = resolve_object_text_v01(input);
            assert_eq!(resolution.resolved_kind, None);
            assert_eq!(
                resolution.diagnostics[0].code,
                "object-text.payload-identity"
            );
        }
    }

    #[test]
    fn reports_unresolved_and_syntax_diagnostics_without_runtime_mapping() {
        let unresolved = resolve_object_text_v01("user.manipulator 1");
        assert_eq!(unresolved.diagnostics[0].code, "object-text.unresolved");

        let invalid = resolve_object_text_v01("[+ 1");
        assert_eq!(invalid.diagnostics[0].code, "object-text.invalid-syntax");

        let empty = resolve_object_text_v01("   ");
        assert_eq!(empty.diagnostics[0].code, "object-text.empty");
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
        assert_eq!(project_entry.canonical_object_text, "my-patcher");
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
            "object-text.invalid-arg-count",
        );
        assert_diagnostic(&registry.resolve("p"), "object-text.invalid-arg-count");
        assert_diagnostic(&registry.resolve("p true"), "object-text.invalid-arg-type");

        let mismatched = construct_project_patch(
            ParsedObjectText {
                input: "p other".to_owned(),
                display_text: "p other".to_owned(),
                class_symbol: "p".to_owned(),
                creation_args: vec![ObjectTextAtom::Symbol("other".to_owned())],
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
        assert_diagnostic(&mismatched, "object-text.unresolved");
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
            "object-text.provider-unavailable",
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
        assert_diagnostic(&ambiguous, "object-text.ambiguous");
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
            "object-text.unresolved",
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
            "object-text.unresolved",
        );
    }

    #[test]
    fn object_text_materialization_and_port_projection_cover_diagnostic_edges() {
        let unresolved = ObjectTextResolution {
            input: "future".to_owned(),
            display_text: "future".to_owned(),
            class_symbol: "future".to_owned(),
            creation_args: vec![
                ObjectTextAtom::Float(1.5),
                ObjectTextAtom::Int(2),
                ObjectTextAtom::Bool(true),
                ObjectTextAtom::Symbol("arg".to_owned()),
            ],
            resolved_kind: None,
            resolved_kind_version: None,
            params: Map::new(),
            instance_ports: Vec::new(),
            candidates: vec![ObjectTextCandidateSummary {
                id: "package:future".to_owned(),
                source: "package-provider".to_owned(),
                kind: "object.future".to_owned(),
                display_name: "Future".to_owned(),
            }],
            diagnostics: Vec::new(),
        };

        let materialize_error = materialize_object_text_node_v01(&unresolved, "future_1")
            .expect_err("unresolved object should not materialize as resolved node");
        assert_eq!(materialize_error.code, "object-text.unresolved");
        assert!(materialize_error.message.contains("future"));

        let diagnostic_node = materialize_unresolved_object_text_node_v01(&unresolved, "future_1");
        assert_eq!(diagnostic_node.kind, "object.core.unresolved");
        assert_eq!(diagnostic_node.params["candidateCount"], json!(1));
        assert_eq!(
            diagnostic_node.params["candidates"][0]["source"],
            "package-provider"
        );

        for (rate, expected_rate) in [
            (ObjectTextPortRate::Event, PortRateCurrent::Event),
            (ObjectTextPortRate::Render, PortRateCurrent::Render),
            (ObjectTextPortRate::Gpu, PortRateCurrent::Gpu),
            (ObjectTextPortRate::Resource, PortRateCurrent::Resource),
            (ObjectTextPortRate::Io, PortRateCurrent::Io),
        ] {
            let current = object_text_port_to_current(&input_port(
                "in",
                "value.core.message",
                rate,
                ObjectTextPortActivation::Passive,
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
}
