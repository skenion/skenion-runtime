use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use crate::{DiagnosticSeverity, RuntimeDiagnostic, contract::ExtensionKind};
use skenion_contracts::{ExtensionManifestV01, validate_node_definition_v01};

pub const RUNTIME_EXTENSION_MANIFEST_FILE: &str = "skenion.extension.json";
pub const RUNTIME_EXTENSION_ABI_VERSION: &str = "0.1.0";
pub const SKENION_EXTENSION_PATH_ENV: &str = "SKENION_EXTENSION_PATH";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeExtensionListResponse {
    pub ok: bool,
    pub extensions: Vec<RuntimeExtensionDescriptor>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeExtensionDescriptor {
    pub id: String,
    pub version: String,
    pub kind: ExtensionKind,
    pub runtime_abi_version: String,
    pub manifest_path: String,
    pub status: RuntimeExtensionStatus,
    pub capabilities: Vec<String>,
    pub provided_nodes: Vec<String>,
    pub provided_codecs: Vec<String>,
    pub provided_transports: Vec<String>,
    pub provided_help: Vec<String>,
    pub test_ids: Vec<String>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeExtensionStatus {
    Loaded,
    Disabled,
    Failed,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeExtensionManager {
    package_dirs: Vec<PathBuf>,
}

impl RuntimeExtensionManager {
    pub fn from_env() -> Self {
        let Some(paths) = env::var_os(SKENION_EXTENSION_PATH_ENV) else {
            return Self::default();
        };

        Self {
            package_dirs: env::split_paths(&paths).collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_package_dirs(package_dirs: Vec<PathBuf>) -> Self {
        Self { package_dirs }
    }

    pub fn list_extensions(&self) -> RuntimeExtensionListResponse {
        let mut diagnostics = Vec::new();
        let extensions = self
            .package_dirs
            .iter()
            .filter_map(|package_dir| {
                let manifest_path = package_dir.join(RUNTIME_EXTENSION_MANIFEST_FILE);
                if !manifest_path.exists() {
                    diagnostics.push(diagnostic(
                        DiagnosticSeverity::Warning,
                        format!(
                            "extension package {} does not contain {RUNTIME_EXTENSION_MANIFEST_FILE}",
                            package_dir.display()
                        ),
                    ));
                    return None;
                }
                Some(read_extension_package(package_dir, &manifest_path))
            })
            .collect::<Vec<_>>();

        let ok = diagnostics
            .iter()
            .chain(
                extensions
                    .iter()
                    .flat_map(|extension| extension.diagnostics.iter()),
            )
            .all(|diagnostic| diagnostic.severity != DiagnosticSeverity::Error);

        RuntimeExtensionListResponse {
            ok,
            extensions,
            diagnostics,
        }
    }
}

impl Default for RuntimeExtensionListResponse {
    fn default() -> Self {
        Self {
            ok: true,
            extensions: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

fn read_extension_package(package_dir: &Path, manifest_path: &Path) -> RuntimeExtensionDescriptor {
    let absolute_manifest_path =
        fs::canonicalize(manifest_path).unwrap_or_else(|_| manifest_path.to_path_buf());
    let manifest = match fs::read_to_string(manifest_path)
        .map_err(|error| format!("failed to read extension manifest: {error}"))
        .and_then(|contents| {
            serde_json::from_str::<ExtensionManifestV01>(&contents)
                .map_err(|error| format!("failed to parse extension manifest: {error}"))
        }) {
        Ok(manifest) => manifest,
        Err(message) => {
            return RuntimeExtensionDescriptor {
                id: package_dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("unknown-extension")
                    .to_owned(),
                version: "0.0.0".to_owned(),
                kind: ExtensionKind::NodePack,
                runtime_abi_version: RUNTIME_EXTENSION_ABI_VERSION.to_owned(),
                manifest_path: absolute_manifest_path.display().to_string(),
                status: RuntimeExtensionStatus::Failed,
                capabilities: Vec::new(),
                provided_nodes: Vec::new(),
                provided_codecs: Vec::new(),
                provided_transports: Vec::new(),
                provided_help: Vec::new(),
                test_ids: Vec::new(),
                diagnostics: vec![diagnostic(DiagnosticSeverity::Error, message)],
            };
        }
    };

    descriptor_from_manifest(package_dir, absolute_manifest_path, manifest)
}

fn descriptor_from_manifest(
    package_dir: &Path,
    manifest_path: PathBuf,
    manifest: ExtensionManifestV01,
) -> RuntimeExtensionDescriptor {
    let mut diagnostics = validate_manifest(package_dir, &manifest);
    let status = if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    {
        RuntimeExtensionStatus::Failed
    } else {
        RuntimeExtensionStatus::Loaded
    };

    let provided_nodes = manifest
        .provides
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let provided_codecs = manifest
        .provides
        .codecs
        .iter()
        .map(|codec| codec.id.clone())
        .collect::<Vec<_>>();
    let provided_transports = manifest
        .provides
        .transports
        .iter()
        .map(|transport| transport.id.clone())
        .collect::<Vec<_>>();
    let provided_help = manifest
        .provides
        .help
        .iter()
        .map(|help| help.node_id.clone())
        .collect::<Vec<_>>();
    let test_ids = manifest
        .tests
        .iter()
        .map(|test| test.id.clone())
        .collect::<Vec<_>>();

    let capabilities = manifest
        .provides
        .nodes
        .iter()
        .flat_map(|node| node.capabilities.iter().cloned())
        .collect::<Vec<_>>();

    RuntimeExtensionDescriptor {
        id: manifest.id,
        version: manifest.version,
        kind: manifest.kind,
        runtime_abi_version: manifest.runtime_abi_version,
        manifest_path: manifest_path.display().to_string(),
        status,
        capabilities,
        provided_nodes,
        provided_codecs,
        provided_transports,
        provided_help,
        test_ids,
        diagnostics: {
            diagnostics.sort_by(|a, b| a.message.cmp(&b.message));
            diagnostics
        },
    }
}

fn validate_manifest(
    package_dir: &Path,
    manifest: &ExtensionManifestV01,
) -> Vec<RuntimeDiagnostic> {
    let mut diagnostics = Vec::new();

    if manifest.schema != "skenion.extension.manifest" {
        diagnostics.push(diagnostic(
            DiagnosticSeverity::Error,
            format!(
                "extension manifest schema must be skenion.extension.manifest, got {}",
                manifest.schema
            ),
        ));
    }
    if manifest.schema_version != "0.1.0" {
        diagnostics.push(diagnostic(
            DiagnosticSeverity::Error,
            format!(
                "extension manifest schemaVersion must be 0.1.0, got {}",
                manifest.schema_version
            ),
        ));
    }
    if manifest.runtime_abi_version != RUNTIME_EXTENSION_ABI_VERSION {
        diagnostics.push(diagnostic(
            DiagnosticSeverity::Error,
            format!(
                "extension {} requires runtimeAbiVersion {}, but runtime supports {RUNTIME_EXTENSION_ABI_VERSION}",
                manifest.id, manifest.runtime_abi_version
            ),
        ));
    }

    for node in &manifest.provides.nodes {
        if let Err(report) = validate_node_definition_v01(node) {
            diagnostics.push(diagnostic(
                DiagnosticSeverity::Error,
                format!("node {} failed contract validation: {}", node.id, report),
            ));
        }
    }

    for help in &manifest.provides.help {
        validate_optional_relative_file(
            package_dir,
            help.markdown_path.as_deref(),
            &mut diagnostics,
        );
        validate_optional_relative_file(package_dir, help.graph_path.as_deref(), &mut diagnostics);
    }

    for test in &manifest.tests {
        validate_optional_relative_file(
            package_dir,
            test.fixture_path.as_deref(),
            &mut diagnostics,
        );
        validate_optional_relative_file(
            package_dir,
            test.expected_path.as_deref(),
            &mut diagnostics,
        );
    }

    match manifest.kind {
        ExtensionKind::NativeRuntime => match &manifest.native {
            Some(native) => validate_native_binding(package_dir, native, &mut diagnostics),
            None => diagnostics.push(diagnostic(
                DiagnosticSeverity::Error,
                "native-runtime extension must declare native binding".to_owned(),
            )),
        },
        ExtensionKind::CorePackage | ExtensionKind::Codec | ExtensionKind::NodePack => {
            if let Some(native) = &manifest.native {
                validate_native_binding(package_dir, native, &mut diagnostics);
            }
        }
    }

    diagnostics
}

fn validate_native_binding(
    package_dir: &Path,
    native: &crate::contract::ExtensionNativeBinding,
    diagnostics: &mut Vec<RuntimeDiagnostic>,
) {
    if native.entrypoint != "skenion_extension_init" {
        diagnostics.push(diagnostic(
            DiagnosticSeverity::Error,
            format!(
                "native entrypoint must be skenion_extension_init, got {}",
                native.entrypoint
            ),
        ));
    }

    let Some(artifact) = native
        .artifacts
        .iter()
        .find(|artifact| artifact.os == env::consts::OS && artifact.arch == env::consts::ARCH)
    else {
        diagnostics.push(diagnostic(
            DiagnosticSeverity::Error,
            format!(
                "native extension has no artifact for {}-{}",
                env::consts::OS,
                env::consts::ARCH
            ),
        ));
        return;
    };

    match relative_package_path(package_dir, &artifact.path) {
        Ok(path) if path.is_file() => {}
        Ok(path) => diagnostics.push(diagnostic(
            DiagnosticSeverity::Error,
            format!("native artifact does not exist: {}", path.display()),
        )),
        Err(message) => diagnostics.push(diagnostic(DiagnosticSeverity::Error, message)),
    }
}

fn validate_optional_relative_file(
    package_dir: &Path,
    relative_path: Option<&str>,
    diagnostics: &mut Vec<RuntimeDiagnostic>,
) {
    let Some(relative_path) = relative_path else {
        return;
    };

    match relative_package_path(package_dir, relative_path) {
        Ok(path) if path.is_file() => {}
        Ok(path) => diagnostics.push(diagnostic(
            DiagnosticSeverity::Error,
            format!("package file does not exist: {}", path.display()),
        )),
        Err(message) => diagnostics.push(diagnostic(DiagnosticSeverity::Error, message)),
    }
}

fn relative_package_path(package_dir: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative_path);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(format!(
            "extension package path must stay inside package directory: {relative_path}"
        ));
    }

    Ok(package_dir.join(path))
}

fn diagnostic(severity: DiagnosticSeverity, message: String) -> RuntimeDiagnostic {
    RuntimeDiagnostic { severity, message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "skenion-runtime-extension-manager-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_manifest(package_dir: &Path, body: &str) {
        fs::write(package_dir.join(RUNTIME_EXTENSION_MANIFEST_FILE), body).unwrap();
    }

    #[test]
    fn package_directory_without_manifest_is_reported_as_warning() {
        let package_dir = temp_dir("missing-manifest");
        let manager = RuntimeExtensionManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_extensions();

        assert!(response.ok);
        assert!(response.extensions.is_empty());
        assert_eq!(
            response.diagnostics[0].severity,
            DiagnosticSeverity::Warning
        );
    }

    #[test]
    fn invalid_node_test_or_help_paths_fail_only_that_extension() {
        let package_dir = temp_dir("invalid-package-paths");
        write_manifest(
            &package_dir,
            r#"{
              "schema": "skenion.extension.manifest",
              "schemaVersion": "0.1.0",
              "id": "example/bad-paths",
              "version": "0.1.0",
              "runtimeAbiVersion": "0.1.0",
              "kind": "node-pack",
              "provides": {
                "help": [
                  { "nodeId": "example.node", "markdownPath": "help/missing.md" }
                ]
              },
              "permissions": [],
              "tests": [
                { "id": "missing-fixture", "kind": "node", "target": "example.node", "fixturePath": "../outside.json" }
              ]
            }"#,
        );
        let manager = RuntimeExtensionManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_extensions();

        assert!(!response.ok);
        assert_eq!(response.extensions.len(), 1);
        assert_eq!(
            response.extensions[0].status,
            RuntimeExtensionStatus::Failed
        );
        assert!(response.extensions[0].diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("must stay inside package directory")
        }));
    }

    #[test]
    fn core_package_loads_through_same_package_contract() {
        let package_dir = temp_dir("core-package");
        fs::create_dir_all(package_dir.join("help")).unwrap();
        fs::create_dir_all(package_dir.join("tests")).unwrap();
        fs::write(package_dir.join("help/value.md"), "# Value").unwrap();
        fs::write(package_dir.join("tests/value.input.json"), "{}").unwrap();
        fs::write(package_dir.join("tests/value.expected.json"), "{}").unwrap();
        write_manifest(
            &package_dir,
            r#"{
              "schema": "skenion.extension.manifest",
              "schemaVersion": "0.1.0",
              "id": "skenion/core",
              "version": "0.1.0",
              "runtimeAbiVersion": "0.1.0",
              "kind": "core-package",
              "provides": {
                "nodes": [
                  {
                    "schema": "skenion.node.definition",
                    "schemaVersion": "0.1.0",
                    "id": "core.value",
                    "version": "0.1.0",
                    "displayName": "Value",
                    "category": "Core",
                    "ports": [
                      { "id": "out", "direction": "output", "type": { "flow": "value", "dataKind": "number.float" } }
                    ],
                    "execution": { "model": "value" },
                    "state": { "persistent": false },
                    "permissions": [],
                    "capabilities": ["value.number.v0.1"]
                  }
                ],
                "help": [
                  { "nodeId": "core.value", "markdownPath": "help/value.md" }
                ]
              },
              "permissions": [],
              "tests": [
                {
                  "id": "value-baseline",
                  "kind": "node",
                  "target": "core.value",
                  "fixturePath": "tests/value.input.json",
                  "expectedPath": "tests/value.expected.json"
                }
              ]
            }"#,
        );
        let manager = RuntimeExtensionManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_extensions();

        assert!(response.ok, "{response:?}");
        assert_eq!(response.extensions[0].id, "skenion/core");
        assert_eq!(
            response.extensions[0].status,
            RuntimeExtensionStatus::Loaded
        );
        assert_eq!(response.extensions[0].provided_nodes, vec!["core.value"]);
        assert_eq!(response.extensions[0].provided_help, vec!["core.value"]);
        assert_eq!(response.extensions[0].test_ids, vec!["value-baseline"]);
    }
}
