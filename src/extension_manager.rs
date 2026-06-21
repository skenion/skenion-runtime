use std::{
    env,
    ffi::OsString,
    fs,
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
        Self::from_extension_paths(env::var_os(SKENION_EXTENSION_PATH_ENV))
    }

    fn from_extension_paths(paths: Option<OsString>) -> Self {
        let Some(paths) = paths else {
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
    RuntimeDiagnostic {
        severity,
        message,
        code: None,
        details: None,
    }
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

    fn current_artifact(path: &str) -> String {
        format!(
            r#"{{ "os": "{}", "arch": "{}", "abi": "c", "path": "{path}" }}"#,
            env::consts::OS,
            env::consts::ARCH
        )
    }

    #[test]
    fn extension_paths_parse_like_runtime_environment() {
        let first = PathBuf::from("/tmp/skenion-extension-one");
        let second = PathBuf::from("/tmp/skenion-extension-two");
        let joined = env::join_paths([&first, &second]).unwrap();

        let manager = RuntimeExtensionManager::from_extension_paths(Some(joined));

        assert_eq!(manager.package_dirs, vec![first, second]);
    }

    #[test]
    fn extension_manager_can_be_created_from_process_environment() {
        let manager = RuntimeExtensionManager::from_env();

        let _ = manager.list_extensions();
    }

    #[test]
    fn default_extension_response_is_empty_and_ok() {
        let response = RuntimeExtensionListResponse::default();

        assert!(response.ok);
        assert!(response.extensions.is_empty());
        assert!(response.diagnostics.is_empty());
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
    fn malformed_manifest_fails_only_that_extension() {
        let package_dir = temp_dir("malformed-manifest");
        write_manifest(&package_dir, "{ not-json");
        let manager = RuntimeExtensionManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_extensions();

        assert!(!response.ok);
        assert!(response.extensions[0].id.contains("malformed-manifest"));
        assert_eq!(
            response.extensions[0].status,
            RuntimeExtensionStatus::Failed
        );
        assert!(
            response.extensions[0].diagnostics[0]
                .message
                .contains("failed to parse extension manifest")
        );
    }

    #[test]
    fn manifest_read_error_reports_failed_extension_with_fallback_path() {
        let package_dir = temp_dir("unreadable-manifest");
        let manifest_path = package_dir.join(RUNTIME_EXTENSION_MANIFEST_FILE);

        let descriptor = read_extension_package(&package_dir, &manifest_path);

        assert_eq!(descriptor.status, RuntimeExtensionStatus::Failed);
        assert_eq!(
            descriptor.manifest_path,
            manifest_path.display().to_string()
        );
        assert!(
            descriptor.diagnostics[0]
                .message
                .contains("failed to read extension manifest")
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
    fn schema_abi_and_native_contract_errors_are_package_diagnostics() {
        let package_dir = temp_dir("invalid-package-contract");
        write_manifest(
            &package_dir,
            r#"{
              "schema": "other.manifest",
              "schemaVersion": "9.9.9",
              "id": "example/native-missing",
              "version": "0.1.0",
              "runtimeAbiVersion": "9.9.9",
              "kind": "native-runtime",
              "provides": {},
              "permissions": []
            }"#,
        );
        let manager = RuntimeExtensionManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_extensions();
        let messages = response.extensions[0]
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();

        assert!(!response.ok);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("schema must"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("schemaVersion must"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("requires runtimeAbiVersion"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("must declare native binding"))
        );
    }

    #[test]
    fn invalid_node_contract_is_reported_as_package_failure() {
        let package_dir = temp_dir("invalid-node-contract");
        write_manifest(
            &package_dir,
            r#"{
              "schema": "skenion.extension.manifest",
              "schemaVersion": "0.1.0",
              "id": "example/invalid-node",
              "version": "0.1.0",
              "runtimeAbiVersion": "0.1.0",
              "kind": "node-pack",
              "provides": {
                "nodes": [
                  {
                    "schema": "skenion.node.definition",
                    "schemaVersion": "9.9.9",
                    "id": "example.bad",
                    "version": "0.1.0",
                    "displayName": "Bad",
                    "category": "Example",
                    "ports": [],
                    "execution": { "model": "value" },
                    "state": { "persistent": false },
                    "permissions": [],
                    "capabilities": []
                  }
                ]
              },
              "permissions": []
            }"#,
        );
        let manager = RuntimeExtensionManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_extensions();

        assert!(!response.ok);
        assert_eq!(
            response.extensions[0].status,
            RuntimeExtensionStatus::Failed
        );
        assert!(response.extensions[0].diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("node example.bad failed contract validation")
        }));
    }

    #[test]
    fn native_binding_validation_reports_missing_and_unsafe_artifacts() {
        let missing_artifact_dir = temp_dir("native-missing-artifact");
        write_manifest(
            &missing_artifact_dir,
            &format!(
                r#"{{
                  "schema": "skenion.extension.manifest",
                  "schemaVersion": "0.1.0",
                  "id": "example/native-missing-artifact",
                  "version": "0.1.0",
                  "runtimeAbiVersion": "0.1.0",
                  "kind": "native-runtime",
                  "native": {{
                    "entrypoint": "bad_init",
                    "artifacts": [{}]
                  }},
                  "provides": {{}},
                  "permissions": []
                }}"#,
                current_artifact("target/release/libmissing.dylib")
            ),
        );

        let unsupported_artifact_dir = temp_dir("native-unsupported-artifact");
        write_manifest(
            &unsupported_artifact_dir,
            r#"{
              "schema": "skenion.extension.manifest",
              "schemaVersion": "0.1.0",
              "id": "example/native-unsupported-artifact",
              "version": "0.1.0",
              "runtimeAbiVersion": "0.1.0",
              "kind": "native-runtime",
              "native": {
                "entrypoint": "skenion_extension_init",
                "artifacts": [
                  { "os": "not-this-os", "arch": "not-this-arch", "abi": "c", "path": "libnative.dylib" }
                ]
              },
              "provides": {},
              "permissions": []
            }"#,
        );

        let unsafe_artifact_dir = temp_dir("native-unsafe-artifact");
        write_manifest(
            &unsafe_artifact_dir,
            &format!(
                r#"{{
                  "schema": "skenion.extension.manifest",
                  "schemaVersion": "0.1.0",
                  "id": "example/native-unsafe-artifact",
                  "version": "0.1.0",
                  "runtimeAbiVersion": "0.1.0",
                  "kind": "native-runtime",
                  "native": {{
                    "entrypoint": "skenion_extension_init",
                    "artifacts": [{}]
                  }},
                  "provides": {{}},
                  "permissions": []
                }}"#,
                current_artifact("../outside/libnative.dylib")
            ),
        );

        let manager = RuntimeExtensionManager::with_package_dirs(vec![
            missing_artifact_dir,
            unsupported_artifact_dir,
            unsafe_artifact_dir,
        ]);

        let response = manager.list_extensions();
        let messages = response
            .extensions
            .iter()
            .flat_map(|extension| extension.diagnostics.iter())
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();

        assert!(!response.ok);
        assert!(messages.iter().any(|message| message.contains("bad_init")));
        assert!(
            messages
                .iter()
                .any(|message| message.contains("native artifact does not exist"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("has no artifact"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("must stay inside package directory"))
        );
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

    #[test]
    fn native_package_with_artifact_loads_codecs_transports_help_and_tests() {
        let package_dir = temp_dir("native-valid");
        fs::create_dir_all(package_dir.join("target/release")).unwrap();
        fs::create_dir_all(package_dir.join("help")).unwrap();
        fs::create_dir_all(package_dir.join("tests")).unwrap();
        fs::write(package_dir.join("target/release/libnative.dylib"), "").unwrap();
        fs::write(package_dir.join("help/native.md"), "# Native").unwrap();
        fs::write(package_dir.join("help/native.graph.json"), "{}").unwrap();
        fs::write(package_dir.join("tests/native.input.json"), "{}").unwrap();
        fs::write(package_dir.join("tests/native.expected.json"), "{}").unwrap();
        write_manifest(
            &package_dir,
            &format!(
                r#"{{
                  "schema": "skenion.extension.manifest",
                  "schemaVersion": "0.1.0",
                  "id": "example/native-valid",
                  "version": "0.1.0",
                  "runtimeAbiVersion": "0.1.0",
                  "kind": "native-runtime",
                  "native": {{
                    "entrypoint": "skenion_extension_init",
                    "artifacts": [{}]
                  }},
                  "provides": {{
                    "codecs": [
                      {{
                        "id": "example.serial.decode",
                        "version": "0.1.0",
                        "transportKinds": ["serial"],
                        "direction": "decode"
                      }}
                    ],
                    "transports": [
                      {{ "id": "example.serial", "version": "0.1.0", "kind": "serial" }}
                    ],
                    "help": [
                      {{
                        "nodeId": "example.native",
                        "markdownPath": "help/native.md",
                        "graphPath": "help/native.graph.json"
                      }}
                    ]
                  }},
                  "permissions": ["io.serial"],
                  "tests": [
                    {{
                      "id": "native-baseline",
                      "kind": "extension",
                      "target": "example/native-valid",
                      "fixturePath": "tests/native.input.json",
                      "expectedPath": "tests/native.expected.json"
                    }}
                  ]
                }}"#,
                current_artifact("target/release/libnative.dylib")
            ),
        );
        let manager = RuntimeExtensionManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_extensions();

        assert!(response.ok, "{response:?}");
        assert_eq!(
            response.extensions[0].status,
            RuntimeExtensionStatus::Loaded
        );
        assert_eq!(
            response.extensions[0].provided_codecs,
            vec!["example.serial.decode"]
        );
        assert_eq!(
            response.extensions[0].provided_transports,
            vec!["example.serial"]
        );
        assert_eq!(response.extensions[0].provided_help, vec!["example.native"]);
        assert_eq!(response.extensions[0].test_ids, vec!["native-baseline"]);
    }

    #[test]
    fn codec_package_can_optionally_declare_native_binding() {
        let package_dir = temp_dir("codec-native-valid");
        fs::create_dir_all(package_dir.join("target/release")).unwrap();
        fs::write(package_dir.join("target/release/libcodec.dylib"), "").unwrap();
        write_manifest(
            &package_dir,
            &format!(
                r#"{{
                  "schema": "skenion.extension.manifest",
                  "schemaVersion": "0.1.0",
                  "id": "example/codec-native-valid",
                  "version": "0.1.0",
                  "runtimeAbiVersion": "0.1.0",
                  "kind": "codec",
                  "native": {{
                    "entrypoint": "skenion_extension_init",
                    "artifacts": [{}]
                  }},
                  "provides": {{
                    "codecs": [
                      {{
                        "id": "example.hid.decode",
                        "version": "0.1.0",
                        "transportKinds": ["hid"],
                        "direction": "decode"
                      }}
                    ]
                  }},
                  "permissions": ["io.hid"]
                }}"#,
                current_artifact("target/release/libcodec.dylib")
            ),
        );
        let manager = RuntimeExtensionManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_extensions();

        assert!(response.ok, "{response:?}");
        assert_eq!(
            response.extensions[0].status,
            RuntimeExtensionStatus::Loaded
        );
        assert_eq!(
            response.extensions[0].provided_codecs,
            vec!["example.hid.decode"]
        );
    }
}
