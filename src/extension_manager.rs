use std::{
    env,
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{DiagnosticSeverity, RuntimeDiagnostic};
use skenion_contracts::{
    ExtensionKindV01 as ExtensionKind, ExtensionManifestV01 as ExtensionManifest,
    ExtensionNativeBindingV01, validate_extension_manifest_v01, validate_node_definition_v01,
};

pub const RUNTIME_EXTENSION_MANIFEST_FILE: &str = "skenion.extension.json";
pub const RUNTIME_EXTENSION_ABI_VERSION: &str = "0.1.0";
pub const SKENION_EXTENSION_PATH_ENV: &str = "SKENION_EXTENSION_PATH";
const EXTENSION_MANIFEST_SCHEMA: &str = "skenion.extension.manifest";
const EXTENSION_MANIFEST_SCHEMA_VERSION: &str = "0.1.0";
const EXTENSION_REGISTRY_SOURCE: &str = "runtime-extension-registry";
const EXTENSION_REGISTRY_ACTION: &str = "scan";
const EXTENSION_REGISTRY_EVENT: &str = "extension-package-load";

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
pub struct RuntimeExtensionRegistrySnapshot {
    response: RuntimeExtensionListResponse,
}

impl RuntimeExtensionRegistrySnapshot {
    pub(crate) fn from_response(response: RuntimeExtensionListResponse) -> Self {
        Self { response }
    }

    pub fn response(&self) -> RuntimeExtensionListResponse {
        self.response.clone()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeExtensionRegistryScan {
    snapshot: RuntimeExtensionRegistrySnapshot,
    log_diagnostics: Vec<RuntimeDiagnostic>,
}

impl RuntimeExtensionRegistryScan {
    pub(crate) fn response(&self) -> RuntimeExtensionListResponse {
        self.snapshot.response()
    }

    pub(crate) fn log_diagnostics(&self) -> &[RuntimeDiagnostic] {
        &self.log_diagnostics
    }

    pub(crate) fn into_snapshot(self) -> RuntimeExtensionRegistrySnapshot {
        self.snapshot
    }
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

    pub(crate) fn scan_registry(&self) -> RuntimeExtensionRegistryScan {
        let mut diagnostics = Vec::new();
        let extensions = self
            .package_dirs
            .iter()
            .filter_map(|package_dir| {
                let manifest_path = package_dir.join(RUNTIME_EXTENSION_MANIFEST_FILE);
                if !manifest_path.exists() {
                    diagnostics.push(RuntimeDiagnostic::structured_warning(
                        "extension.manifest.missing",
                        format!(
                            "extension package {} does not contain {RUNTIME_EXTENSION_MANIFEST_FILE}",
                            package_dir.display()
                        ),
                        registry_diagnostic_details(
                            "extension-manifest",
                            package_dir,
                            Some(&manifest_path),
                            None,
                            None,
                            json!({
                                "expectedManifestFile": RUNTIME_EXTENSION_MANIFEST_FILE,
                            }),
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

        let response = RuntimeExtensionListResponse {
            ok,
            extensions,
            diagnostics,
        };
        let log_diagnostics = registry_log_diagnostics(&response);

        RuntimeExtensionRegistryScan {
            snapshot: RuntimeExtensionRegistrySnapshot::from_response(response),
            log_diagnostics,
        }
    }

    pub fn list_extensions(&self) -> RuntimeExtensionListResponse {
        self.scan_registry().response()
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
    let contents = match fs::read_to_string(manifest_path) {
        Ok(contents) => contents,
        Err(error) => {
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
                diagnostics: vec![RuntimeDiagnostic::structured_error(
                    "extension.manifest.read-failed",
                    format!("failed to read extension manifest: {error}"),
                    registry_diagnostic_details(
                        "extension-manifest",
                        package_dir,
                        Some(&absolute_manifest_path),
                        None,
                        None,
                        json!({
                            "error": error.to_string(),
                        }),
                    ),
                )],
            };
        }
    };

    let manifest_value = match serde_json::from_str::<serde_json::Value>(&contents) {
        Ok(value) => value,
        Err(error) => {
            return failed_descriptor_from_manifest_value(
                package_dir,
                &absolute_manifest_path,
                None,
                RuntimeDiagnostic::structured_error(
                    "extension.manifest.parse-failed",
                    format!("failed to parse extension manifest: {error}"),
                    registry_diagnostic_details(
                        "extension-manifest",
                        package_dir,
                        Some(&absolute_manifest_path),
                        None,
                        None,
                        json!({
                            "error": error.to_string(),
                        }),
                    ),
                ),
            );
        }
    };

    let manifest = match serde_json::from_value::<ExtensionManifest>(manifest_value.clone()) {
        Ok(manifest) => manifest,
        Err(error) => {
            let header = manifest_header_from_value(&manifest_value);
            return failed_descriptor_from_manifest_value(
                package_dir,
                &absolute_manifest_path,
                header.as_ref(),
                RuntimeDiagnostic::structured_error(
                    "extension.manifest.parse-failed",
                    format!("failed to parse extension manifest: {error}"),
                    header_diagnostic_details(
                        package_dir,
                        &absolute_manifest_path,
                        header.as_ref(),
                        "extension-manifest",
                        json!({
                            "error": error.to_string(),
                        }),
                    ),
                ),
            );
        }
    };

    descriptor_from_manifest(package_dir, absolute_manifest_path, manifest)
}

fn descriptor_from_manifest(
    package_dir: &Path,
    manifest_path: PathBuf,
    manifest: ExtensionManifest,
) -> RuntimeExtensionDescriptor {
    let mut diagnostics = validate_manifest(package_dir, &manifest_path, &manifest);
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
    manifest_path: &Path,
    manifest: &ExtensionManifest,
) -> Vec<RuntimeDiagnostic> {
    let mut diagnostics = Vec::new();

    if let Err(report) = validate_extension_manifest_v01(manifest) {
        diagnostics.extend(report.errors().iter().map(|error| {
            RuntimeDiagnostic::structured_error(
                "extension.manifest.contract-invalid",
                format!(
                    "extension manifest failed contract validation: {}",
                    error.message
                ),
                manifest_diagnostic_details(
                    package_dir,
                    manifest_path,
                    manifest,
                    "extension-manifest",
                    json!({
                        "error": error.message,
                    }),
                ),
            )
        }));
    }

    if manifest.schema != EXTENSION_MANIFEST_SCHEMA {
        diagnostics.push(RuntimeDiagnostic::structured_error(
            "extension.manifest.invalid-schema",
            format!(
                "extension manifest schema must be skenion.extension.manifest, got {}",
                manifest.schema
            ),
            manifest_diagnostic_details(
                package_dir,
                manifest_path,
                manifest,
                "extension-manifest",
                json!({
                    "expectedSchema": EXTENSION_MANIFEST_SCHEMA,
                    "receivedSchema": manifest.schema,
                }),
            ),
        ));
    }
    if manifest.schema_version != EXTENSION_MANIFEST_SCHEMA_VERSION {
        diagnostics.push(RuntimeDiagnostic::structured_error(
            "extension.manifest.unsupported-schema-version",
            format!(
                "extension manifest schemaVersion must be 0.1.0, got {}",
                manifest.schema_version
            ),
            manifest_diagnostic_details(
                package_dir,
                manifest_path,
                manifest,
                "extension-manifest",
                json!({
                    "expectedSchemaVersion": EXTENSION_MANIFEST_SCHEMA_VERSION,
                    "receivedSchemaVersion": manifest.schema_version,
                }),
            ),
        ));
    }
    if manifest.runtime_abi_version != RUNTIME_EXTENSION_ABI_VERSION {
        diagnostics.push(RuntimeDiagnostic::structured_error(
            "extension.runtime-abi.unsupported-version",
            format!(
                "extension {} requires runtimeAbiVersion {}, but runtime supports {RUNTIME_EXTENSION_ABI_VERSION}",
                manifest.id, manifest.runtime_abi_version
            ),
            manifest_diagnostic_details(
                package_dir,
                manifest_path,
                manifest,
                "extension-runtime-abi",
                json!({
                    "expectedRuntimeAbiVersion": RUNTIME_EXTENSION_ABI_VERSION,
                    "receivedRuntimeAbiVersion": manifest.runtime_abi_version,
                }),
            ),
        ));
    }

    for node in &manifest.provides.nodes {
        if let Err(report) = validate_node_definition_v01(node) {
            for error in report.errors() {
                diagnostics.push(RuntimeDiagnostic::structured_error(
                    "extension.node.contract-invalid",
                    format!(
                        "node {} failed contract validation: {}",
                        node.id, error.message
                    ),
                    manifest_diagnostic_details(
                        package_dir,
                        manifest_path,
                        manifest,
                        "extension-node-definition",
                        json!({
                            "nodeId": node.id,
                            "nodeVersion": node.version,
                            "error": error.message,
                        }),
                    ),
                ));
            }
        }
    }

    for help in &manifest.provides.help {
        validate_optional_relative_file(
            package_dir,
            manifest_path,
            manifest,
            help.markdown_path.as_deref(),
            &mut diagnostics,
            HELP_MARKDOWN_FILE,
            json!({
                "nodeId": help.node_id,
            }),
        );
        validate_optional_relative_file(
            package_dir,
            manifest_path,
            manifest,
            help.graph_path.as_deref(),
            &mut diagnostics,
            HELP_GRAPH_FILE,
            json!({
                "nodeId": help.node_id,
            }),
        );
    }

    for test in &manifest.tests {
        validate_optional_relative_file(
            package_dir,
            manifest_path,
            manifest,
            test.fixture_path.as_deref(),
            &mut diagnostics,
            TEST_FIXTURE_FILE,
            json!({
                "testId": test.id,
                "target": test.target,
            }),
        );
        validate_optional_relative_file(
            package_dir,
            manifest_path,
            manifest,
            test.expected_path.as_deref(),
            &mut diagnostics,
            TEST_EXPECTED_FILE,
            json!({
                "testId": test.id,
                "target": test.target,
            }),
        );
    }

    match manifest.kind {
        ExtensionKind::NativeRuntime => match &manifest.native {
            Some(native) => validate_native_binding(
                package_dir,
                manifest_path,
                manifest,
                native,
                &mut diagnostics,
            ),
            None => diagnostics.push(RuntimeDiagnostic::structured_error(
                "extension.native.missing-binding",
                "native-runtime extension must declare native binding",
                manifest_diagnostic_details(
                    package_dir,
                    manifest_path,
                    manifest,
                    "extension-native-binding",
                    json!({}),
                ),
            )),
        },
        ExtensionKind::CorePackage | ExtensionKind::Codec | ExtensionKind::NodePack => {
            if let Some(native) = &manifest.native {
                validate_native_binding(
                    package_dir,
                    manifest_path,
                    manifest,
                    native,
                    &mut diagnostics,
                );
            }
        }
    }

    diagnostics
}

fn validate_native_binding(
    package_dir: &Path,
    manifest_path: &Path,
    manifest: &ExtensionManifest,
    native: &ExtensionNativeBindingV01,
    diagnostics: &mut Vec<RuntimeDiagnostic>,
) {
    if native.entrypoint != "skenion_extension_init" {
        diagnostics.push(RuntimeDiagnostic::structured_error(
            "extension.native.invalid-entrypoint",
            format!(
                "native entrypoint must be skenion_extension_init, got {}",
                native.entrypoint
            ),
            manifest_diagnostic_details(
                package_dir,
                manifest_path,
                manifest,
                "extension-native-binding",
                json!({
                    "expectedEntrypoint": "skenion_extension_init",
                    "receivedEntrypoint": native.entrypoint,
                }),
            ),
        ));
    }

    let Some(artifact) = native
        .artifacts
        .iter()
        .find(|artifact| artifact.os == env::consts::OS && artifact.arch == env::consts::ARCH)
    else {
        diagnostics.push(RuntimeDiagnostic::structured_error(
            "extension.native.missing-platform-artifact",
            format!(
                "native extension has no artifact for {}-{}",
                env::consts::OS,
                env::consts::ARCH
            ),
            manifest_diagnostic_details(
                package_dir,
                manifest_path,
                manifest,
                "extension-native-artifact",
                json!({
                    "targetOs": env::consts::OS,
                    "targetArch": env::consts::ARCH,
                }),
            ),
        ));
        return;
    };

    match relative_package_path(package_dir, &artifact.path) {
        Ok(path) if path.is_file() => {}
        Ok(path) => diagnostics.push(RuntimeDiagnostic::structured_error(
            "extension.native.missing-artifact",
            format!("native artifact does not exist: {}", path.display()),
            package_file_diagnostic_details(
                package_dir,
                manifest_path,
                manifest,
                &artifact.path,
                Some(&path),
                NATIVE_ARTIFACT_FILE,
                json!({
                    "targetOs": artifact.os,
                    "targetArch": artifact.arch,
                    "artifactAbi": artifact.abi,
                }),
            ),
        )),
        Err(message) => diagnostics.push(RuntimeDiagnostic::structured_error(
            "extension.package.invalid-path",
            message,
            package_file_diagnostic_details(
                package_dir,
                manifest_path,
                manifest,
                &artifact.path,
                None,
                NATIVE_ARTIFACT_FILE,
                json!({
                    "targetOs": artifact.os,
                    "targetArch": artifact.arch,
                    "artifactAbi": artifact.abi,
                }),
            ),
        )),
    }
}

fn validate_optional_relative_file(
    package_dir: &Path,
    manifest_path: &Path,
    manifest: &ExtensionManifest,
    relative_path: Option<&str>,
    diagnostics: &mut Vec<RuntimeDiagnostic>,
    context: PackageFileDiagnosticContext,
    extra_details: Value,
) {
    let Some(relative_path) = relative_path else {
        return;
    };

    match relative_package_path(package_dir, relative_path) {
        Ok(path) if path.is_file() => {}
        Ok(path) => diagnostics.push(RuntimeDiagnostic::structured_error(
            context.missing_code,
            format!("package file does not exist: {}", path.display()),
            package_file_diagnostic_details(
                package_dir,
                manifest_path,
                manifest,
                relative_path,
                Some(&path),
                context,
                extra_details,
            ),
        )),
        Err(message) => diagnostics.push(RuntimeDiagnostic::structured_error(
            "extension.package.invalid-path",
            message,
            package_file_diagnostic_details(
                package_dir,
                manifest_path,
                manifest,
                relative_path,
                None,
                context,
                extra_details,
            ),
        )),
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

#[derive(Debug, Clone)]
struct ManifestHeader {
    id: String,
    version: String,
    runtime_abi_version: String,
    kind: ExtensionKind,
}

#[derive(Clone, Copy)]
struct PackageFileDiagnosticContext {
    surface: &'static str,
    missing_code: &'static str,
    file_kind: &'static str,
    path_detail_key: &'static str,
}

const HELP_MARKDOWN_FILE: PackageFileDiagnosticContext = PackageFileDiagnosticContext {
    surface: "extension-help-file",
    missing_code: "extension.help.missing-file",
    file_kind: "help-markdown",
    path_detail_key: "filePath",
};

const HELP_GRAPH_FILE: PackageFileDiagnosticContext = PackageFileDiagnosticContext {
    surface: "extension-help-file",
    missing_code: "extension.help.missing-file",
    file_kind: "help-graph",
    path_detail_key: "filePath",
};

const TEST_FIXTURE_FILE: PackageFileDiagnosticContext = PackageFileDiagnosticContext {
    surface: "extension-test-file",
    missing_code: "extension.test.missing-file",
    file_kind: "test-fixture",
    path_detail_key: "filePath",
};

const TEST_EXPECTED_FILE: PackageFileDiagnosticContext = PackageFileDiagnosticContext {
    surface: "extension-test-file",
    missing_code: "extension.test.missing-file",
    file_kind: "test-expected",
    path_detail_key: "filePath",
};

const NATIVE_ARTIFACT_FILE: PackageFileDiagnosticContext = PackageFileDiagnosticContext {
    surface: "extension-native-artifact",
    missing_code: "extension.native.missing-artifact",
    file_kind: "native-artifact",
    path_detail_key: "artifactPath",
};

fn registry_log_diagnostics(response: &RuntimeExtensionListResponse) -> Vec<RuntimeDiagnostic> {
    response
        .diagnostics
        .iter()
        .chain(
            response
                .extensions
                .iter()
                .flat_map(|extension| extension.diagnostics.iter()),
        )
        .filter(|diagnostic| {
            matches!(
                diagnostic.severity,
                DiagnosticSeverity::Warning | DiagnosticSeverity::Error
            )
        })
        .cloned()
        .collect()
}

fn manifest_diagnostic_details(
    package_dir: &Path,
    manifest_path: &Path,
    manifest: &ExtensionManifest,
    surface: &str,
    extra_details: Value,
) -> Value {
    registry_diagnostic_details(
        surface,
        package_dir,
        Some(manifest_path),
        Some(&manifest.id),
        Some(&manifest.version),
        extra_details,
    )
}

fn header_diagnostic_details(
    package_dir: &Path,
    manifest_path: &Path,
    header: Option<&ManifestHeader>,
    surface: &str,
    extra_details: Value,
) -> Value {
    registry_diagnostic_details(
        surface,
        package_dir,
        Some(manifest_path),
        header.map(|header| header.id.as_str()),
        header.map(|header| header.version.as_str()),
        extra_details,
    )
}

fn package_file_diagnostic_details(
    package_dir: &Path,
    manifest_path: &Path,
    manifest: &ExtensionManifest,
    relative_path: &str,
    resolved_path: Option<&Path>,
    context: PackageFileDiagnosticContext,
    extra_details: Value,
) -> Value {
    let mut details = object_details(extra_details);
    details.insert("fileKind".to_owned(), json!(context.file_kind));
    details.insert("relativePath".to_owned(), json!(relative_path));
    details.insert(context.path_detail_key.to_owned(), json!(relative_path));
    if let Some(resolved_path) = resolved_path {
        details.insert(
            "resolvedPath".to_owned(),
            json!(resolved_path.display().to_string()),
        );
    }
    manifest_diagnostic_details(
        package_dir,
        manifest_path,
        manifest,
        context.surface,
        Value::Object(details),
    )
}

fn registry_diagnostic_details(
    surface: &str,
    package_dir: &Path,
    manifest_path: Option<&Path>,
    package_id: Option<&str>,
    package_version: Option<&str>,
    extra_details: Value,
) -> Value {
    let mut details = object_details(extra_details);
    details.insert("surface".to_owned(), json!(surface));
    details.insert("source".to_owned(), json!(EXTENSION_REGISTRY_SOURCE));
    details.insert("action".to_owned(), json!(EXTENSION_REGISTRY_ACTION));
    details.insert("registryEvent".to_owned(), json!(EXTENSION_REGISTRY_EVENT));
    details.insert(
        "packagePath".to_owned(),
        json!(package_dir.display().to_string()),
    );
    if let Some(manifest_path) = manifest_path {
        details.insert(
            "manifestPath".to_owned(),
            json!(manifest_path.display().to_string()),
        );
    }
    if let Some(package_id) = package_id {
        details.insert("packageId".to_owned(), json!(package_id));
        details.insert("manifestId".to_owned(), json!(package_id));
    }
    if let Some(package_version) = package_version {
        details.insert("packageVersion".to_owned(), json!(package_version));
    }
    Value::Object(details)
}

fn object_details(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object,
        _ => Map::new(),
    }
}

fn manifest_header_from_value(value: &serde_json::Value) -> Option<ManifestHeader> {
    let object = value.as_object()?;
    let kind = object
        .get("kind")
        .cloned()
        .and_then(|value| serde_json::from_value::<ExtensionKind>(value).ok())
        .unwrap_or(ExtensionKind::NodePack);
    Some(ManifestHeader {
        id: object
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown-extension")
            .to_owned(),
        version: object
            .get("version")
            .and_then(|value| value.as_str())
            .unwrap_or("0.0.0")
            .to_owned(),
        runtime_abi_version: object
            .get("runtimeAbiVersion")
            .and_then(|value| value.as_str())
            .unwrap_or(RUNTIME_EXTENSION_ABI_VERSION)
            .to_owned(),
        kind,
    })
}

fn failed_descriptor_from_manifest_value(
    package_dir: &Path,
    manifest_path: &Path,
    header: Option<&ManifestHeader>,
    diagnostic: RuntimeDiagnostic,
) -> RuntimeExtensionDescriptor {
    RuntimeExtensionDescriptor {
        id: header
            .map(|header| header.id.clone())
            .or_else(|| {
                package_dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "unknown-extension".to_owned()),
        version: header
            .map(|header| header.version.clone())
            .unwrap_or_else(|| "0.0.0".to_owned()),
        kind: header
            .map(|header| header.kind.clone())
            .unwrap_or(ExtensionKind::NodePack),
        runtime_abi_version: header
            .map(|header| header.runtime_abi_version.clone())
            .unwrap_or_else(|| RUNTIME_EXTENSION_ABI_VERSION.to_owned()),
        manifest_path: manifest_path.display().to_string(),
        status: RuntimeExtensionStatus::Failed,
        capabilities: Vec::new(),
        provided_nodes: Vec::new(),
        provided_codecs: Vec::new(),
        provided_transports: Vec::new(),
        provided_help: Vec::new(),
        test_ids: Vec::new(),
        diagnostics: vec![diagnostic],
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

    fn diagnostic_by_code<'a>(
        diagnostics: &'a [RuntimeDiagnostic],
        code: &str,
    ) -> &'a RuntimeDiagnostic {
        diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code.as_deref() == Some(code))
            .unwrap_or_else(|| panic!("missing {code} diagnostic: {diagnostics:#?}"))
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
        assert_eq!(
            response.diagnostics[0].code.as_deref(),
            Some("extension.manifest.missing")
        );
        let details = response.diagnostics[0].details.as_ref().unwrap();
        assert_eq!(details["surface"], "extension-manifest");
        assert_eq!(details["action"], "scan");
        assert_eq!(details["registryEvent"], "extension-package-load");
        assert_eq!(details["source"], "runtime-extension-registry");
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
    fn invalid_manifest_shape_preserves_header_in_failed_descriptor() {
        let package_dir = temp_dir("header-invalid-manifest");
        write_manifest(
            &package_dir,
            r#"{
              "id": "example/header-invalid",
              "version": "1.2.3",
              "runtimeAbiVersion": "9.9.9",
              "kind": "codec"
            }"#,
        );
        let manager = RuntimeExtensionManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_extensions();

        assert!(!response.ok);
        assert_eq!(response.extensions.len(), 1);
        let descriptor = &response.extensions[0];
        assert_eq!(descriptor.id, "example/header-invalid");
        assert_eq!(descriptor.version, "1.2.3");
        assert_eq!(descriptor.runtime_abi_version, "9.9.9");
        assert_eq!(descriptor.kind, ExtensionKind::Codec);
        assert_eq!(descriptor.status, RuntimeExtensionStatus::Failed);
        let diagnostic =
            diagnostic_by_code(&descriptor.diagnostics, "extension.manifest.parse-failed");
        let details = diagnostic.details.as_ref().unwrap();
        assert_eq!(details["packageId"], "example/header-invalid");
        assert_eq!(details["manifestId"], "example/header-invalid");
        assert_eq!(details["packageVersion"], "1.2.3");
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
    fn unsupported_manifest_schema_version_is_structured_package_failure() {
        let package_dir = temp_dir("legacy-v01-manifest");
        write_manifest(
            &package_dir,
            r#"{
              "schema": "skenion.extension.manifest",
              "schemaVersion": "9.9.9",
              "id": "example/legacy",
              "version": "0.1.0",
              "runtimeAbiVersion": "0.1.0",
              "kind": "node-pack",
              "provides": {},
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
        let diagnostic = diagnostic_by_code(
            &response.extensions[0].diagnostics,
            "extension.manifest.unsupported-schema-version",
        );
        assert_eq!(
            diagnostic.details.as_ref().unwrap()["receivedSchemaVersion"],
            "9.9.9"
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
        let missing_help = diagnostic_by_code(
            &response.extensions[0].diagnostics,
            "extension.help.missing-file",
        );
        assert_eq!(
            missing_help.details.as_ref().unwrap()["fileKind"],
            "help-markdown"
        );
        assert_eq!(
            missing_help.details.as_ref().unwrap()["filePath"],
            "help/missing.md"
        );
        let invalid_package_path = diagnostic_by_code(
            &response.extensions[0].diagnostics,
            "extension.package.invalid-path",
        );
        assert_eq!(
            invalid_package_path.details.as_ref().unwrap()["fileKind"],
            "test-fixture"
        );
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
        let manager = RuntimeExtensionManager::with_package_dirs(vec![package_dir.clone()]);

        let response = manager.list_extensions();
        let diagnostics = &response.extensions[0].diagnostics;
        let messages = diagnostics
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

        let schema_diagnostic =
            diagnostic_by_code(diagnostics, "extension.manifest.invalid-schema");
        let schema_details = schema_diagnostic.details.as_ref().unwrap();
        assert_eq!(schema_details["surface"], "extension-manifest");
        assert_eq!(schema_details["packageId"], "example/native-missing");
        assert_eq!(schema_details["manifestId"], "example/native-missing");
        assert_eq!(
            schema_details["packagePath"],
            package_dir.display().to_string()
        );
        assert_eq!(
            schema_details["manifestPath"],
            response.extensions[0].manifest_path
        );
        assert_eq!(
            schema_details["expectedSchema"],
            "skenion.extension.manifest"
        );
        assert_eq!(schema_details["receivedSchema"], "other.manifest");

        let schema_version_diagnostic =
            diagnostic_by_code(diagnostics, "extension.manifest.unsupported-schema-version");
        let schema_version_details = schema_version_diagnostic.details.as_ref().unwrap();
        assert_eq!(schema_version_details["surface"], "extension-manifest");
        assert_eq!(
            schema_version_details["packageId"],
            "example/native-missing"
        );
        assert_eq!(
            schema_version_details["manifestId"],
            "example/native-missing"
        );
        assert_eq!(
            schema_version_details["packagePath"],
            package_dir.display().to_string()
        );
        assert_eq!(
            schema_version_details["manifestPath"],
            response.extensions[0].manifest_path
        );
        assert_eq!(schema_version_details["expectedSchemaVersion"], "0.1.0");
        assert_eq!(schema_version_details["receivedSchemaVersion"], "9.9.9");

        let abi_diagnostic =
            diagnostic_by_code(diagnostics, "extension.runtime-abi.unsupported-version");
        let abi_details = abi_diagnostic.details.as_ref().unwrap();
        assert_eq!(abi_details["surface"], "extension-runtime-abi");
        assert_eq!(abi_details["packageId"], "example/native-missing");
        assert_eq!(abi_details["manifestId"], "example/native-missing");
        assert_eq!(
            abi_details["packagePath"],
            package_dir.display().to_string()
        );
        assert_eq!(
            abi_details["manifestPath"],
            response.extensions[0].manifest_path
        );
        assert_eq!(
            abi_details["expectedRuntimeAbiVersion"],
            RUNTIME_EXTENSION_ABI_VERSION
        );
        assert_eq!(abi_details["receivedRuntimeAbiVersion"], "9.9.9");

        let missing_native_binding =
            diagnostic_by_code(diagnostics, "extension.native.missing-binding");
        assert_eq!(
            missing_native_binding.details.as_ref().unwrap()["surface"],
            "extension-native-binding"
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
                    "execution": { "model": "control" },
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

        let missing_artifact = response
            .extensions
            .iter()
            .flat_map(|extension| extension.diagnostics.iter())
            .find(|diagnostic| {
                diagnostic.code.as_deref() == Some("extension.native.missing-artifact")
            })
            .expect("missing native artifact diagnostic should exist");
        assert_eq!(
            missing_artifact.details.as_ref().unwrap()["targetOs"],
            env::consts::OS
        );
        assert_eq!(
            missing_artifact.details.as_ref().unwrap()["targetArch"],
            env::consts::ARCH
        );
        assert_eq!(
            missing_artifact.details.as_ref().unwrap()["artifactPath"],
            "target/release/libmissing.dylib"
        );

        let unsupported_artifact = response
            .extensions
            .iter()
            .flat_map(|extension| extension.diagnostics.iter())
            .find(|diagnostic| {
                diagnostic.code.as_deref() == Some("extension.native.missing-platform-artifact")
            })
            .expect("missing platform artifact diagnostic should exist");
        assert_eq!(
            unsupported_artifact.details.as_ref().unwrap()["targetOs"],
            env::consts::OS
        );
        assert_eq!(
            unsupported_artifact.details.as_ref().unwrap()["targetArch"],
            env::consts::ARCH
        );

        let invalid_entrypoint = response
            .extensions
            .iter()
            .flat_map(|extension| extension.diagnostics.iter())
            .find(|diagnostic| {
                diagnostic.code.as_deref() == Some("extension.native.invalid-entrypoint")
            })
            .expect("invalid entrypoint diagnostic should exist");
        assert_eq!(
            invalid_entrypoint.details.as_ref().unwrap()["receivedEntrypoint"],
            "bad_init"
        );
    }

    #[test]
    fn core_package_loads_through_same_package_contract() {
        let package_dir = temp_dir("core-package");
        fs::create_dir_all(package_dir.join("help")).unwrap();
        fs::create_dir_all(package_dir.join("tests")).unwrap();
        fs::write(package_dir.join("help/float.md"), "# Float").unwrap();
        fs::write(package_dir.join("tests/float.input.json"), "{}").unwrap();
        fs::write(package_dir.join("tests/float.expected.json"), "{}").unwrap();
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
                    "id": "object.core.float",
                    "version": "0.1.0",
                    "displayName": "Float",
                    "category": "Typed Controls",
                    "ports": [
                      {
                        "id": "in",
                        "direction": "input",
                        "type": "value.core.message",
                        "rate": "control",
                        "required": false,
                        "triggerMode": "trigger",
                        "accepts": [
                          "value.core.float32",
                          "value.core.int32",
                          "value.core.uint32",
                          "value.core.bool",
                          "value.core.bang"
                        ],
                        "messageKeys": {
                          "accepted": ["bang", "set", "float", "int", "uint", "bool"],
                          "silent": ["set"],
                          "trigger": ["bang", "float", "int", "uint", "bool"],
                          "store": ["set", "float", "int", "uint", "bool"],
                          "emit": ["bang", "float", "int", "uint", "bool"]
                        }
                      },
                      {
                        "id": "cold",
                        "direction": "input",
                        "type": "value.core.float32",
                        "rate": "control",
                        "required": false,
                        "triggerMode": "passive"
                      },
                      {
                        "id": "value",
                        "direction": "output",
                        "type": "value.core.float32",
                        "rate": "control"
                      }
                    ],
                    "execution": { "model": "control" },
                    "state": { "persistent": false },
                    "permissions": [],
                    "capabilities": ["value.core.float32.v0.1"]
                  }
                ],
                "help": [
                  { "nodeId": "object.core.float", "markdownPath": "help/float.md" }
                ]
              },
              "permissions": [],
              "tests": [
                {
                  "id": "float-baseline",
                  "kind": "node",
                  "target": "object.core.float",
                  "fixturePath": "tests/float.input.json",
                  "expectedPath": "tests/float.expected.json"
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
        assert_eq!(
            response.extensions[0].provided_nodes,
            vec!["object.core.float"]
        );
        assert_eq!(
            response.extensions[0].provided_help,
            vec!["object.core.float"]
        );
        assert_eq!(response.extensions[0].test_ids, vec!["float-baseline"]);
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

    #[test]
    fn registry_scan_log_diagnostics_include_only_warning_and_error_package_events() {
        let valid_dir = temp_dir("scan-valid-package");
        write_manifest(
            &valid_dir,
            r#"{
              "schema": "skenion.extension.manifest",
              "schemaVersion": "0.1.0",
              "id": "example/scan-valid",
              "version": "0.1.0",
              "runtimeAbiVersion": "0.1.0",
              "kind": "node-pack",
              "provides": {},
              "permissions": []
            }"#,
        );
        let missing_manifest_dir = temp_dir("scan-missing-manifest");
        let manager =
            RuntimeExtensionManager::with_package_dirs(vec![valid_dir, missing_manifest_dir]);

        let scan = manager.scan_registry();
        let response = scan.response();

        assert!(response.ok);
        assert_eq!(response.extensions.len(), 1);
        assert_eq!(scan.log_diagnostics().len(), 1);
        assert_eq!(
            scan.log_diagnostics()[0].code.as_deref(),
            Some("extension.manifest.missing")
        );
        assert!(scan.log_diagnostics()[0].details.is_some());
    }

    #[test]
    fn package_file_diagnostic_details_include_relative_and_resolved_paths() {
        let package_dir = temp_dir("file-diagnostic-details");
        let manifest_path = package_dir.join(RUNTIME_EXTENSION_MANIFEST_FILE);
        let manifest: ExtensionManifest = serde_json::from_value(json!({
            "schema": "skenion.extension.manifest",
            "schemaVersion": "0.1.0",
            "id": "example/file-details",
            "version": "0.1.0",
            "runtimeAbiVersion": "0.1.0",
            "kind": "node-pack",
            "provides": {},
            "permissions": []
        }))
        .unwrap();
        let resolved_path = package_dir.join("help/example.skenion.json");

        let details = package_file_diagnostic_details(
            &package_dir,
            &manifest_path,
            &manifest,
            "help/example.skenion.json",
            Some(&resolved_path),
            HELP_GRAPH_FILE,
            json!("ignored-non-object"),
        );

        assert_eq!(details["surface"], "extension-help-file");
        assert_eq!(details["fileKind"], "help-graph");
        assert_eq!(details["relativePath"], "help/example.skenion.json");
        assert_eq!(details["filePath"], "help/example.skenion.json");
        assert_eq!(details["resolvedPath"], resolved_path.display().to_string());
        assert_eq!(details["packageId"], "example/file-details");
        assert_eq!(details["packageVersion"], "0.1.0");
        assert_eq!(details["manifestPath"], manifest_path.display().to_string());
    }
}
