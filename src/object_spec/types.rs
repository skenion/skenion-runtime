use serde_json::{Map, Value};
use skenion_contracts::{MessageKeyPolicyV01, PackageChecksumV01};
use std::path::PathBuf;

use crate::{
    ObjectImplementationRefCurrent, ObjectProviderRefCurrent, ObjectResolutionCurrent,
    nodes::CoreNodeImplementation,
};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ObjectSpecResolution {
    pub(crate) input: String,
    pub(crate) display_text: String,
    pub(crate) class_symbol: String,
    pub(crate) creation_args: Vec<ObjectSpecAtom>,
    pub(crate) implementation: Option<ObjectImplementationRefCurrent>,
    pub(crate) object_resolution: ObjectResolutionCurrent,
    pub(crate) params: Map<String, Value>,
    pub(crate) instance_ports: Vec<ObjectSpecPort>,
    pub(crate) candidates: Vec<ObjectSpecCandidateSummary>,
    pub(crate) issues: Vec<ObjectSpecIssue>,
}

impl ObjectSpecResolution {
    pub(crate) fn ok(&self) -> bool {
        self.issues.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectSpecCandidateSummary {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) implementation: ObjectImplementationRefCurrent,
    pub(crate) object_spec: Option<String>,
    pub(crate) display_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ObjectSpecAtom {
    Float(f64),
    Int(i64),
    Bool(bool),
    Symbol(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectSpecIssue {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectSpecPort {
    pub(crate) id: String,
    pub(crate) direction: ObjectSpecPortDirection,
    pub(crate) port_type: String,
    pub(crate) label: Option<String>,
    pub(crate) rate: ObjectSpecPortRate,
    pub(crate) accepts: Option<Vec<String>>,
    pub(crate) activation: Option<ObjectSpecPortActivation>,
    pub(crate) message_keys: Option<MessageKeyPolicyV01>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectSpecPortDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectSpecPortRate {
    Event,
    Control,
    Audio,
    Render,
    Gpu,
    Resource,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectSpecPortActivation {
    Trigger,
    Latched,
    Passive,
}

#[derive(Clone)]
pub(crate) struct ObjectRegistry {
    pub(super) candidates: Vec<ObjectRegistryCandidate>,
    pub(super) allow_unchecked_project_patch_refs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) enum ObjectRegistrySource {
    FirstPartyCore,
    ProjectPatch,
    PackageProvider,
    NativeProvider,
}

#[derive(Clone)]
pub(crate) struct ObjectRegistryCandidate {
    pub(super) id: String,
    pub(super) source: ObjectRegistrySource,
    pub(super) aliases: Vec<String>,
    pub(super) implementation: ObjectImplementationRefCurrent,
    pub(super) executable_kind: String,
    pub(super) display_name: String,
    pub(super) core: Option<&'static dyn CoreNodeImplementation>,
    pub(super) catalog_category: Option<&'static str>,
    pub(super) project_patch: Option<ProjectPatchCandidate>,
    pub(super) package: Option<PackageObjectCandidate>,
}

#[derive(Debug, Clone)]
pub(super) struct ProjectPatchCandidate {
    pub(super) patch_id: String,
    pub(super) revision: String,
    pub(super) description: Option<String>,
    pub(super) interface_digest: PackageChecksumV01,
    pub(super) ports: Vec<ObjectSpecPort>,
}

#[derive(Debug, Clone)]
pub(super) struct PackageObjectCandidate {
    pub(super) package_id: String,
    pub(super) root_path: Option<PathBuf>,
    pub(super) definition_path: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedObjectSpec {
    pub(super) input: String,
    pub(super) display_text: String,
    pub(super) class_symbol: String,
    pub(super) creation_args: Vec<ObjectSpecAtom>,
}

pub(super) fn core_implementation(object_id: impl Into<String>) -> ObjectImplementationRefCurrent {
    ObjectImplementationRefCurrent {
        provider: ObjectProviderRefCurrent::Core,
        object_id: object_id.into(),
        interface_digest: None,
    }
}
