use skenion_contracts::{
    NodeCatalogDisplayPaletteV01, NodeCatalogDisplayV01, NodeCatalogEntryV01,
    NodeCatalogSnapshotV01, PackageChecksumAlgorithmV01, PackageChecksumV01,
};
use std::fs;

use crate::{
    NodeDefinitionCurrent, ObjectImplementationRefCurrent, ObjectProviderRefCurrent,
    PackageRegistryListResponseV01, PatchDefinitionCurrent, PortDirectionCurrent, PortRateCurrent,
    PortSpecCurrent, ProjectDocumentCurrent,
    nodes::{CoreNodeImplementation, first_party_core_nodes},
};

mod ports;
mod projection;
mod resolver;
mod types;

pub(crate) use types::{
    ObjectRegistry, ObjectRegistryCandidate, ObjectSpecAtom, ObjectSpecCandidateSummary,
    ObjectSpecIssue, ObjectSpecPort, ObjectSpecPortActivation, ObjectSpecPortDirection,
    ObjectSpecPortRate, ObjectSpecResolution, ParsedObjectSpec,
};

use types::ObjectRegistrySource;
use types::{PackageObjectCandidate, ProjectPatchCandidate, core_implementation};

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

    pub(crate) fn for_project_with_packages(
        project: Option<&ProjectDocumentCurrent>,
        packages: Option<&PackageRegistryListResponseV01>,
    ) -> Self {
        let mut registry = Self::for_project(project);
        if let Some(packages) = packages {
            registry.register_package_objects(packages);
        }
        registry
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
        let mut entries = self
            .candidates
            .iter()
            .filter_map(|candidate| match candidate.source {
                ObjectRegistrySource::FirstPartyCore => self.core_catalog_entry(candidate),
                ObjectRegistrySource::ProjectPatch => project_patch_catalog_entry(candidate),
                ObjectRegistrySource::PackageProvider => package_catalog_entry(candidate),
                ObjectRegistrySource::NativeProvider => None,
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.catalog_id.cmp(&right.catalog_id));

        let mut snapshot = NodeCatalogSnapshotV01 {
            schema: "skenion.node-catalog.snapshot".to_owned(),
            schema_version: CURRENT_KIND_VERSION.to_owned(),
            catalog_revision: zero_catalog_revision_checksum(),
            entries,
            issues: None,
        };
        snapshot.catalog_revision = skenion_contracts::compute_node_catalog_revision_v01(&snapshot);
        snapshot
    }

    pub(crate) fn node_definition_projection(&self) -> Vec<NodeDefinitionCurrent> {
        self.catalog_projection()
            .entries
            .into_iter()
            .map(|entry| entry.definition)
            .collect()
    }

    fn core_catalog_entry(
        &self,
        candidate: &ObjectRegistryCandidate,
    ) -> Option<NodeCatalogEntryV01> {
        if candidate.executable_kind == "object.core.subpatch" {
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
            object_id: candidate.implementation.object_id.clone(),
            primary_object_spec: canonical_object_spec,
            aliases: None,
            provider: ObjectProviderRefCurrent::Core,
            definition,
            creatable: true,
            display: NodeCatalogDisplayV01 {
                title: candidate.display_name.clone(),
                category: Some(core_catalog_category(candidate).to_owned()),
                palette: Some(NodeCatalogDisplayPaletteV01::Text),
                description: None,
                help_id: Some(candidate.executable_kind.clone()),
            },
            issues: None,
        })
    }

    fn register_first_party_core(&mut self) {
        for node in first_party_core_nodes() {
            self.register_core_candidate(*node);
        }
    }

    fn register_core_candidate(&mut self, node: &'static dyn CoreNodeImplementation) {
        let mut aliases = node
            .aliases()
            .iter()
            .map(|alias| (*alias).to_owned())
            .collect::<Vec<_>>();
        if !aliases.iter().any(|alias| alias == node.object_id()) {
            aliases.push(node.object_id().to_owned());
        }

        self.candidates.push(ObjectRegistryCandidate {
            id: node.kind().to_owned(),
            source: ObjectRegistrySource::FirstPartyCore,
            aliases,
            implementation: core_implementation(node.object_id()),
            executable_kind: node.kind().to_owned(),
            display_name: node.display_name().to_owned(),
            core: Some(node),
            catalog_category: Some(node.catalog_category()),
            project_patch: None,
            package: None,
        });
    }

    fn register_project_patches(&mut self, patch_library: &[PatchDefinitionCurrent]) {
        for patch in patch_library {
            let kind = project_patch_object_kind(&patch.id);
            self.candidates.push(ObjectRegistryCandidate {
                id: format!("project-patch:{}", patch.id),
                source: ObjectRegistrySource::ProjectPatch,
                aliases: vec![patch.id.clone()],
                implementation: ObjectImplementationRefCurrent {
                    provider: ObjectProviderRefCurrent::ProjectPatch {
                        patch_id: patch.id.clone(),
                        revision: Some(patch.revision.clone()),
                        interface_revision: None,
                        interface_digest: Some(
                            skenion_contracts::compute_patch_interface_digest_v01(patch),
                        ),
                    },
                    object_id: patch.id.clone(),
                    interface_digest: Some(skenion_contracts::compute_patch_interface_digest_v01(
                        patch,
                    )),
                },
                executable_kind: kind,
                display_name: patch
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.title.clone())
                    .unwrap_or_else(|| patch.id.clone()),
                core: None,
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
                package: None,
            });
        }
    }

    fn register_package_objects(&mut self, packages: &PackageRegistryListResponseV01) {
        for package in &packages.packages {
            for object in &package.provides.objects {
                let mut aliases = Vec::with_capacity(object.aliases.len() + 1);
                aliases.push(object.primary_object_spec.clone());
                aliases.extend(object.aliases.clone());
                self.candidates.push(ObjectRegistryCandidate {
                    id: format!("package:{}:{}", package.package_id, object.object_id),
                    source: ObjectRegistrySource::PackageProvider,
                    aliases,
                    implementation: ObjectImplementationRefCurrent {
                        provider: ObjectProviderRefCurrent::Package {
                            package_id: package.package_id.clone(),
                            lock_entry_id: None,
                            version: Some(package.version.clone()),
                        },
                        object_id: object.object_id.clone(),
                        interface_digest: None,
                    },
                    executable_kind: object.object_id.clone(),
                    display_name: object.object_id.clone(),
                    core: None,
                    catalog_category: None,
                    project_patch: None,
                    package: Some(PackageObjectCandidate {
                        package_id: package.package_id.clone(),
                        root_path: package.root_path.clone(),
                        definition_path: object.definition_path.clone(),
                    }),
                });
            }
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
            implementation: core_implementation("subpatch"),
            executable_kind: "object.core.subpatch".to_owned(),
            display_name: patch_id.clone(),
            core: self
                .core_candidate("object.core.subpatch")
                .and_then(|candidate| candidate.core),
            catalog_category: Some("Core"),
            project_patch: Some(ProjectPatchCandidate {
                patch_id,
                revision: CURRENT_KIND_VERSION.to_owned(),
                description: None,
                interface_digest: zero_catalog_revision_checksum(),
                ports: Vec::new(),
            }),
            package: None,
        }]
    }

    fn core_candidate(&self, kind: &str) -> Option<ObjectRegistryCandidate> {
        self.candidates
            .iter()
            .find(|candidate| {
                candidate.source == ObjectRegistrySource::FirstPartyCore
                    && candidate.executable_kind == kind
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
            ObjectRegistrySource::PackageProvider => construct_package_object(parsed, candidate),
            ObjectRegistrySource::NativeProvider => failure(
                &parsed.input,
                parsed.display_text,
                &parsed.class_symbol,
                parsed.creation_args,
                "object-spec.provider-unavailable",
                "package and native object providers are reserved but not loaded in this Runtime tranche",
            ),
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
            implementation: self.implementation.clone(),
            object_spec: self.canonical_object_spec(),
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
        object_id: candidate.implementation.object_id.clone(),
        primary_object_spec: patch.patch_id.clone(),
        aliases: None,
        provider: ObjectProviderRefCurrent::ProjectPatch {
            patch_id: patch.patch_id.clone(),
            revision: None,
            interface_revision: None,
            interface_digest: Some(patch.interface_digest.clone()),
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
        issues: None,
    })
}

fn package_catalog_entry(candidate: &ObjectRegistryCandidate) -> Option<NodeCatalogEntryV01> {
    let package = candidate.package.as_ref()?;
    let definition = load_package_object_definition(candidate)?;
    let primary_object_spec = candidate.canonical_object_spec()?;
    let aliases = candidate
        .aliases
        .iter()
        .filter(|alias| *alias != &primary_object_spec)
        .cloned()
        .collect::<Vec<_>>();

    Some(NodeCatalogEntryV01 {
        catalog_id: format!(
            "package.{}.{}",
            catalog_slug(&package.package_id),
            catalog_slug(&candidate.implementation.object_id)
        ),
        object_id: candidate.implementation.object_id.clone(),
        primary_object_spec,
        aliases: (!aliases.is_empty()).then_some(aliases),
        provider: candidate.implementation.provider.clone(),
        definition,
        creatable: true,
        display: NodeCatalogDisplayV01 {
            title: candidate.display_name.clone(),
            category: Some("Package".to_owned()),
            palette: Some(NodeCatalogDisplayPaletteV01::Direct),
            description: None,
            help_id: None,
        },
        issues: None,
    })
}

pub(crate) fn load_package_object_definition(
    candidate: &ObjectRegistryCandidate,
) -> Option<NodeDefinitionCurrent> {
    let package = candidate.package.as_ref()?;
    let root_path = package.root_path.as_ref()?;
    let definition_path = root_path.join(&package.definition_path);
    fs::read_to_string(definition_path)
        .ok()
        .and_then(|contents| serde_json::from_str::<NodeDefinitionCurrent>(&contents).ok())
}

fn catalog_slug(input: &str) -> String {
    let slug = input
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if slug.is_empty() {
        "object".to_owned()
    } else {
        slug
    }
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
        .executable_kind
        .strip_prefix("object.core.")
        .unwrap_or(candidate.executable_kind.as_str());
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

pub(crate) fn object_spec_port_from_current(port: &PortSpecCurrent) -> ObjectSpecPort {
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
pub(crate) use projection::materialize_unresolved_object_spec_node_v01;
use projection::object_spec_port_to_current;
pub(crate) use projection::{materialize_object_spec_node_v01, object_spec_node_definition_v01};
use resolver::{
    ambiguous_resolution, construct_first_party_core, construct_package_object,
    construct_project_patch, explicit_project_patch_ref, failure, parse_object_spec_input_v01,
    unresolved_resolution, unsupported_first_party_audio_message,
};
#[cfg(test)]
use resolver::{contract_object_spec_atom_to_runtime, runtime_object_spec_issue_code};
pub(crate) use resolver::{
    resolve_core_audio, resolve_core_boundary_port, resolve_core_control_operator,
    resolve_core_control_value, resolve_core_subpatch,
};

#[cfg(test)]
mod tests;
