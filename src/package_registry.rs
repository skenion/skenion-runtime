use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use skenion_contracts::{
    PackageChecksumAlgorithmV01, PackageChecksumV01, PackageContractsRequirementV01,
    PackageIssueSeverityV01, PackageIssueV01, PackageManifestV01, PackageObjectExportV01,
    PackageProvidedRefV01, PackageProvidesV01, PackageRootKindV01, PackageSourceV01,
    PackageTargetTripleV01, PackageTrustV01, SKENION_PACKAGE_MANIFEST_FILE_NAME,
    validate_package_manifest_v01,
};

pub const RUNTIME_PACKAGE_MANIFEST_FILE: &str = SKENION_PACKAGE_MANIFEST_FILE_NAME;
pub const SKENION_PACKAGE_PATH_ENV: &str = "SKENION_PACKAGE_PATH";
const PACKAGE_REGISTRY_SOURCE: &str = "runtime-package-registry";
const PACKAGE_REGISTRY_ACTION: &str = "scan";
const PACKAGE_REGISTRY_EVENT: &str = "package-registry-load";

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PackageRegistryEntryV01 {
    pub package_id: String,
    pub version: String,
    pub category: skenion_contracts::PackageCategoryV01,
    pub source: PackageSourceV01,
    pub root: PackageRootKindV01,
    pub trust: PackageTrustV01,
    pub contracts: PackageContractsRequirementV01,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_abi_range: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<PackageTargetTripleV01>,
    pub manifest_path: String,
    #[serde(skip)]
    pub(crate) root_path: Option<PathBuf>,
    pub manifest_checksum: PackageChecksumV01,
    pub provides: PackageProvidesV01,
    pub issues: Vec<PackageIssueV01>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PackageRegistryListResponseV01 {
    pub ok: bool,
    pub packages: Vec<PackageRegistryEntryV01>,
    pub issues: Vec<PackageIssueV01>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimePackageRegistryState {
    Empty,
    Ready,
    Degraded,
}

#[derive(Debug, Clone)]
pub struct RuntimePackageRegistrySnapshot {
    response: PackageRegistryListResponseV01,
    revision: u64,
    event_id: String,
    state: RuntimePackageRegistryState,
}

impl Default for RuntimePackageRegistrySnapshot {
    fn default() -> Self {
        Self {
            response: PackageRegistryListResponseV01 {
                ok: true,
                packages: Vec::new(),
                issues: Vec::new(),
            },
            revision: 0,
            event_id: "package-registry-event-000000".to_owned(),
            state: RuntimePackageRegistryState::Empty,
        }
    }
}

impl RuntimePackageRegistrySnapshot {
    fn from_response(response: PackageRegistryListResponseV01) -> Self {
        let state = if has_error(&response) {
            RuntimePackageRegistryState::Degraded
        } else if response.packages.is_empty() {
            RuntimePackageRegistryState::Empty
        } else {
            RuntimePackageRegistryState::Ready
        };

        Self {
            response,
            revision: 1,
            event_id: "package-registry-event-000001".to_owned(),
            state,
        }
    }

    pub fn response(&self) -> PackageRegistryListResponseV01 {
        self.response.clone()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn event_id(&self) -> &str {
        &self.event_id
    }

    pub fn state(&self) -> RuntimePackageRegistryState {
        self.state
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimePackageRegistryScan {
    snapshot: RuntimePackageRegistrySnapshot,
}

impl RuntimePackageRegistryScan {
    pub(crate) fn into_snapshot(self) -> RuntimePackageRegistrySnapshot {
        self.snapshot
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimePackageManager {
    package_dirs: Vec<PathBuf>,
}

impl RuntimePackageManager {
    pub fn from_env() -> Self {
        Self::from_package_paths(env::var_os(SKENION_PACKAGE_PATH_ENV))
    }

    fn from_package_paths(paths: Option<OsString>) -> Self {
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

    pub(crate) fn scan_registry(&self) -> RuntimePackageRegistryScan {
        let root_infos = self.root_infos();
        let mut issues = root_overlap_issues(&root_infos);
        let duplicate_root_indexes = duplicate_root_indexes(&root_infos);
        let mut packages = Vec::new();

        for root_info in root_infos {
            if duplicate_root_indexes.contains(&root_info.index) {
                continue;
            }

            match read_package_root(&root_info) {
                PackageRootRead::Package(entry) => packages.push(*entry),
                PackageRootRead::Issues(mut root_issues) => {
                    issues.append(&mut root_issues);
                }
            }
        }

        issues.extend(duplicate_package_issues(&packages));
        sort_package_issues(&mut issues);
        packages.sort_by(|a, b| {
            a.package_id
                .cmp(&b.package_id)
                .then_with(|| a.version.cmp(&b.version))
        });

        let response = PackageRegistryListResponseV01 {
            ok: !issues
                .iter()
                .chain(packages.iter().flat_map(|package| package.issues.iter()))
                .any(package_issue_is_error),
            packages,
            issues,
        };
        RuntimePackageRegistryScan {
            snapshot: RuntimePackageRegistrySnapshot::from_response(response),
        }
    }

    pub fn list_packages(&self) -> PackageRegistryListResponseV01 {
        self.scan_registry().into_snapshot().response()
    }

    fn root_infos(&self) -> Vec<PackageRootInfo> {
        self.package_dirs
            .iter()
            .enumerate()
            .map(|(index, path)| PackageRootInfo {
                index,
                path: path.clone(),
                canonical_path: fs::canonicalize(path).ok(),
                display_name: package_root_display_name(index, path),
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct PackageRootInfo {
    index: usize,
    path: PathBuf,
    canonical_path: Option<PathBuf>,
    display_name: String,
}

enum PackageRootRead {
    Package(Box<PackageRegistryEntryV01>),
    Issues(Vec<PackageIssueV01>),
}

fn read_package_root(root_info: &PackageRootInfo) -> PackageRootRead {
    let Some(root_canonical) = root_info.canonical_path.as_deref() else {
        return PackageRootRead::Issues(vec![root_issue(
            PackageIssueSeverityV01::Error,
            "package.root.unreadable",
            format!("package root {} is not readable", root_info.display_name),
            root_info,
            None,
            json!({}),
        )]);
    };

    let manifest_path = root_info.path.join(RUNTIME_PACKAGE_MANIFEST_FILE);
    let extension_manifest_path = root_info
        .path
        .join(crate::extension_manager::RUNTIME_EXTENSION_MANIFEST_FILE);
    let has_package_manifest = manifest_path.is_file();
    let has_extension_manifest = extension_manifest_path.is_file();

    if has_package_manifest && has_extension_manifest {
        return PackageRootRead::Issues(vec![root_issue(
            PackageIssueSeverityV01::Error,
            "package.root.both-manifests",
            format!(
                "package root {} must not contain both skenion.package.json and skenion.extension.json",
                root_info.display_name
            ),
            root_info,
            Some(RUNTIME_PACKAGE_MANIFEST_FILE),
            json!({
                "legacyManifestPath": crate::extension_manager::RUNTIME_EXTENSION_MANIFEST_FILE,
            }),
        )]);
    }

    if !has_package_manifest && has_extension_manifest {
        return PackageRootRead::Issues(vec![root_issue(
            PackageIssueSeverityV01::Error,
            "package.root.extension-only",
            format!(
                "package root {} contains only the legacy extension manifest",
                root_info.display_name
            ),
            root_info,
            Some(crate::extension_manager::RUNTIME_EXTENSION_MANIFEST_FILE),
            json!({
                "expectedManifestPath": RUNTIME_PACKAGE_MANIFEST_FILE,
            }),
        )]);
    }

    if !has_package_manifest {
        return PackageRootRead::Issues(vec![root_issue(
            PackageIssueSeverityV01::Error,
            "package.manifest.missing",
            format!(
                "package root {} does not contain skenion.package.json",
                root_info.display_name
            ),
            root_info,
            Some(RUNTIME_PACKAGE_MANIFEST_FILE),
            json!({
                "expectedManifestPath": RUNTIME_PACKAGE_MANIFEST_FILE,
            }),
        )]);
    }

    let contents = match fs::read_to_string(&manifest_path) {
        Ok(contents) => contents,
        Err(error) => {
            return PackageRootRead::Issues(vec![root_issue(
                PackageIssueSeverityV01::Error,
                "package.manifest.read-failed",
                format!(
                    "failed to read package manifest for {}",
                    root_info.display_name
                ),
                root_info,
                Some(RUNTIME_PACKAGE_MANIFEST_FILE),
                json!({
                    "error": error.to_string(),
                }),
            )]);
        }
    };

    let manifest_value = match serde_json::from_str::<Value>(&contents) {
        Ok(value) => value,
        Err(error) => {
            return PackageRootRead::Issues(vec![root_issue(
                PackageIssueSeverityV01::Error,
                "package.manifest.parse-failed",
                format!(
                    "failed to parse package manifest for {}",
                    root_info.display_name
                ),
                root_info,
                Some(RUNTIME_PACKAGE_MANIFEST_FILE),
                json!({
                    "error": error.to_string(),
                }),
            )]);
        }
    };

    let manifest = match serde_json::from_value::<PackageManifestV01>(manifest_value) {
        Ok(manifest) => manifest,
        Err(error) => {
            return PackageRootRead::Issues(vec![root_issue(
                PackageIssueSeverityV01::Error,
                "package.manifest.decode-failed",
                format!(
                    "failed to decode package manifest for {}",
                    root_info.display_name
                ),
                root_info,
                Some(RUNTIME_PACKAGE_MANIFEST_FILE),
                json!({
                    "error": error.to_string(),
                }),
            )]);
        }
    };

    PackageRootRead::Package(Box::new(package_entry_from_manifest(
        root_info,
        root_canonical,
        &contents,
        manifest,
    )))
}

fn package_entry_from_manifest(
    root_info: &PackageRootInfo,
    root_canonical: &Path,
    manifest_contents: &str,
    manifest: PackageManifestV01,
) -> PackageRegistryEntryV01 {
    let mut issues = manifest.issues.clone();
    issues.extend(contract_validation_issues(root_info, &manifest));
    issues.extend(package_path_issues(root_info, root_canonical, &manifest));
    sort_package_issues(&mut issues);

    PackageRegistryEntryV01 {
        package_id: manifest.id,
        version: manifest.version,
        category: manifest.category,
        source: PackageSourceV01::Workspace,
        root: PackageRootKindV01::Package,
        trust: PackageTrustV01::Trusted,
        contracts: manifest.contracts,
        runtime_abi_range: manifest.runtime_abi_range,
        targets: manifest.targets,
        manifest_path: RUNTIME_PACKAGE_MANIFEST_FILE.to_owned(),
        root_path: Some(root_canonical.to_path_buf()),
        manifest_checksum: manifest_checksum(manifest_contents.as_bytes()),
        provides: public_package_provides(manifest.provides),
        issues,
    }
}

fn public_package_provides(mut provides: PackageProvidesV01) -> PackageProvidesV01 {
    sanitize_provided_paths(&mut provides.patches);
    sanitize_provided_paths(&mut provides.nodes);
    sanitize_object_definition_paths(&mut provides.objects);
    sanitize_provided_paths(&mut provides.resources);
    sanitize_provided_paths(&mut provides.help);
    provides
}

fn sanitize_provided_paths(provided: &mut [PackageProvidedRefV01]) {
    for provided in provided {
        provided.path = public_manifest_path(&provided.path);
    }
}

fn sanitize_object_definition_paths(objects: &mut [PackageObjectExportV01]) {
    for object in objects {
        object.definition_path = public_manifest_path(&object.definition_path);
    }
}

fn contract_validation_issues(
    root_info: &PackageRootInfo,
    manifest: &PackageManifestV01,
) -> Vec<PackageIssueV01> {
    match validate_package_manifest_v01(manifest) {
        Ok(()) => Vec::new(),
        Err(report) => report
            .errors()
            .iter()
            .map(|error| {
                manifest_issue(
                    PackageIssueSeverityV01::Error,
                    "package.manifest.contract-invalid",
                    format!(
                        "package manifest failed contract validation: {}",
                        error.message
                    ),
                    root_info,
                    manifest,
                    json!({
                        "error": error.message,
                    }),
                )
            })
            .collect(),
    }
}

fn package_path_issues(
    root_info: &PackageRootInfo,
    root_canonical: &Path,
    manifest: &PackageManifestV01,
) -> Vec<PackageIssueV01> {
    let mut issues = Vec::new();
    let mut check_path = |relative_path: &str, path_kind: &'static str| {
        if let Some(issue) = validate_package_relative_path(
            root_info,
            root_canonical,
            manifest,
            relative_path,
            path_kind,
        ) {
            issues.push(issue);
        }
    };

    for provided in &manifest.provides.patches {
        check_path(&provided.path, "provided-patch");
    }
    for provided in &manifest.provides.nodes {
        check_path(&provided.path, "provided-node");
    }
    for object in &manifest.provides.objects {
        check_path(&object.definition_path, "provided-object-definition");
    }
    for provided in &manifest.provides.resources {
        check_path(&provided.path, "provided-resource");
    }
    for provided in &manifest.provides.help {
        check_path(&provided.path, "provided-help");
    }
    for path in &manifest.paths.patches {
        check_path(path, "patch-path");
    }
    for path in &manifest.paths.resources {
        check_path(path, "resource-path");
    }
    for path in &manifest.paths.docs {
        check_path(path, "docs-path");
    }
    for path in &manifest.paths.tests {
        check_path(path, "test-path");
    }
    for checksum in &manifest.checksums {
        check_path(&checksum.path, "checksum-path");
    }
    for evidence in &manifest.evidence {
        check_path(&evidence.path, "evidence-path");
    }
    for artifact in &manifest.native_artifacts {
        check_path(&artifact.path, "native-artifact-path");
    }

    issues
}

fn validate_package_relative_path(
    root_info: &PackageRootInfo,
    root_canonical: &Path,
    manifest: &PackageManifestV01,
    relative_path: &str,
    path_kind: &'static str,
) -> Option<PackageIssueV01> {
    let path = Path::new(relative_path);
    if relative_path.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Some(manifest_issue(
            PackageIssueSeverityV01::Error,
            "package.path.invalid",
            "package path must stay inside package root",
            root_info,
            manifest,
            json!({
                "pathKind": path_kind,
                "path": public_manifest_path(relative_path),
                "pathViolation": package_path_violation(relative_path),
            }),
        ));
    }

    let candidate = root_info.path.join(path);
    match fs::canonicalize(&candidate) {
        Ok(resolved_path) if !resolved_path.starts_with(root_canonical) => Some(manifest_issue(
            PackageIssueSeverityV01::Error,
            "package.path.symlink-escape",
            format!("package path escapes package root through a symlink: {relative_path}"),
            root_info,
            manifest,
            json!({
                "pathKind": path_kind,
                "relativePath": relative_path,
            }),
        )),
        Ok(_) | Err(_) => None,
    }
}

fn root_overlap_issues(root_infos: &[PackageRootInfo]) -> Vec<PackageIssueV01> {
    let mut issues = Vec::new();
    for (left_index, left) in root_infos.iter().enumerate() {
        let Some(left_canonical) = left.canonical_path.as_deref() else {
            continue;
        };
        for right in root_infos.iter().skip(left_index + 1) {
            let Some(right_canonical) = right.canonical_path.as_deref() else {
                continue;
            };
            if left_canonical == right_canonical {
                issues.push(root_issue(
                    PackageIssueSeverityV01::Error,
                    "package.root.duplicate",
                    format!(
                        "package root {} duplicates another configured package root",
                        right.display_name
                    ),
                    right,
                    None,
                    json!({
                        "otherRootIndex": left.index,
                        "otherRootName": left.display_name,
                    }),
                ));
            } else if left_canonical.starts_with(right_canonical)
                || right_canonical.starts_with(left_canonical)
            {
                issues.push(root_issue(
                    PackageIssueSeverityV01::Error,
                    "package.root.overlap",
                    format!(
                        "package root {} overlaps another configured package root",
                        right.display_name
                    ),
                    right,
                    None,
                    json!({
                        "otherRootIndex": left.index,
                        "otherRootName": left.display_name,
                    }),
                ));
            }
        }
    }
    issues
}

fn duplicate_root_indexes(root_infos: &[PackageRootInfo]) -> HashSet<usize> {
    let mut seen = HashMap::<&Path, usize>::new();
    let mut duplicate_indexes = HashSet::new();
    for root_info in root_infos {
        let Some(canonical_path) = root_info.canonical_path.as_deref() else {
            continue;
        };
        if seen.insert(canonical_path, root_info.index).is_some() {
            duplicate_indexes.insert(root_info.index);
        }
    }
    duplicate_indexes
}

fn duplicate_package_issues(packages: &[PackageRegistryEntryV01]) -> Vec<PackageIssueV01> {
    let mut issues = Vec::new();
    let mut seen = HashMap::<(&str, &str), usize>::new();
    for (index, package) in packages.iter().enumerate() {
        let key = (package.package_id.as_str(), package.version.as_str());
        if let Some(previous_index) = seen.insert(key, index) {
            issues.push(package_issue(
                PackageIssueSeverityV01::Error,
                "package.registry.duplicate-package",
                format!(
                    "package registry contains duplicate package {}@{}",
                    package.package_id, package.version
                ),
                registry_issue_details(
                    "package-registry-entry",
                    None,
                    Some(&package.package_id),
                    Some(&package.version),
                    json!({
                        "packageIndex": index,
                        "otherPackageIndex": previous_index,
                    }),
                ),
            ));
        }
    }
    issues
}

fn has_error(response: &PackageRegistryListResponseV01) -> bool {
    response
        .issues
        .iter()
        .chain(
            response
                .packages
                .iter()
                .flat_map(|package| package.issues.iter()),
        )
        .any(package_issue_is_error)
}

fn package_issue_is_error(issue: &PackageIssueV01) -> bool {
    issue.severity == PackageIssueSeverityV01::Error
}

fn public_manifest_path(path: &str) -> String {
    if Path::new(path).is_absolute() {
        "<redacted:absolute-path>".to_owned()
    } else {
        path.to_owned()
    }
}

fn package_path_violation(path: &str) -> &'static str {
    let path = Path::new(path);
    if path.as_os_str().is_empty() {
        "empty"
    } else if path.is_absolute() {
        "absolute"
    } else if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        "parent-directory"
    } else {
        "unknown"
    }
}

fn root_issue(
    severity: PackageIssueSeverityV01,
    code: impl Into<String>,
    message: impl Into<String>,
    root_info: &PackageRootInfo,
    manifest_path: Option<&str>,
    extra_details: Value,
) -> PackageIssueV01 {
    package_issue(
        severity,
        code,
        message,
        registry_issue_details(
            "package-root",
            manifest_path,
            None,
            None,
            root_details(root_info, extra_details),
        ),
    )
}

fn manifest_issue(
    severity: PackageIssueSeverityV01,
    code: impl Into<String>,
    message: impl Into<String>,
    root_info: &PackageRootInfo,
    manifest: &PackageManifestV01,
    extra_details: Value,
) -> PackageIssueV01 {
    package_issue(
        severity,
        code,
        message,
        registry_issue_details(
            "package-manifest",
            Some(RUNTIME_PACKAGE_MANIFEST_FILE),
            Some(&manifest.id),
            Some(&manifest.version),
            root_details(root_info, extra_details),
        ),
    )
}

fn package_issue(
    severity: PackageIssueSeverityV01,
    code: impl Into<String>,
    message: impl Into<String>,
    details: Value,
) -> PackageIssueV01 {
    PackageIssueV01 {
        severity,
        code: code.into(),
        message: message.into(),
        details: Some(details),
    }
}

fn registry_issue_details(
    surface: &str,
    manifest_path: Option<&str>,
    package_id: Option<&str>,
    package_version: Option<&str>,
    extra_details: Value,
) -> Value {
    let mut details = object_details(extra_details);
    details.insert("surface".to_owned(), json!(surface));
    details.insert("source".to_owned(), json!(PACKAGE_REGISTRY_SOURCE));
    details.insert("action".to_owned(), json!(PACKAGE_REGISTRY_ACTION));
    details.insert("registryEvent".to_owned(), json!(PACKAGE_REGISTRY_EVENT));
    if let Some(manifest_path) = manifest_path {
        details.insert("manifestPath".to_owned(), json!(manifest_path));
    }
    if let Some(package_id) = package_id {
        details.insert("packageId".to_owned(), json!(package_id));
    }
    if let Some(package_version) = package_version {
        details.insert("packageVersion".to_owned(), json!(package_version));
    }
    Value::Object(details)
}

fn root_details(root_info: &PackageRootInfo, extra_details: Value) -> Value {
    let mut details = object_details(extra_details);
    details.insert("rootIndex".to_owned(), json!(root_info.index));
    details.insert("packageRoot".to_owned(), json!("<redacted>"));
    details.insert("packageRootName".to_owned(), json!(root_info.display_name));
    Value::Object(details)
}

fn object_details(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object,
        _ => Map::new(),
    }
}

fn package_root_display_name(index: usize, path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("package-root-{index}"))
}

fn manifest_checksum(bytes: &[u8]) -> PackageChecksumV01 {
    let digest = Sha256::digest(bytes);
    PackageChecksumV01 {
        algorithm: PackageChecksumAlgorithmV01::Sha256,
        value: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
    }
}

fn sort_package_issues(issues: &mut [PackageIssueV01]) {
    issues.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.message.cmp(&right.message))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io, path::Path};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "skenion-runtime-package-registry-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn valid_manifest(package_id: &str) -> String {
        let provided_id = package_id.replace('/', ".");
        let contracts_version = skenion_contracts::CONTRACTS_PACKAGE_VERSION;
        format!(
            r#"{{
              "schema": "skenion.package.manifest",
              "schemaVersion": "0.1.0",
              "id": "{package_id}",
              "version": "0.49.0",
              "category": "patch",
              "contracts": {{ "version": "{contracts_version}" }},
              "provides": {{
                "patches": [{{ "id": "{provided_id}.main", "path": "patches/main.skenion.json" }}]
              }},
              "paths": {{ "patches": ["patches/main.skenion.json"] }},
              "checksums": [
                {{
                  "id": "manifest",
                  "path": "skenion.package.json",
                  "checksum": {{
                    "algorithm": "sha256",
                    "value": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                  }}
                }}
              ],
              "evidence": [
                {{
                  "id": "manifest-checksum",
                  "kind": "checksum",
                  "path": "evidence/manifest.sha256",
                  "checksum": {{
                    "algorithm": "sha256",
                    "value": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                  }}
                }}
              ]
            }}"#
        )
    }

    fn write_package_manifest(package_dir: &Path, body: &str) {
        fs::write(package_dir.join(RUNTIME_PACKAGE_MANIFEST_FILE), body).unwrap();
    }

    fn write_extension_manifest(package_dir: &Path) {
        fs::write(
            package_dir.join(crate::extension_manager::RUNTIME_EXTENSION_MANIFEST_FILE),
            r#"{
              "schema": "skenion.extension.manifest",
              "schemaVersion": "0.1.0",
              "id": "legacy/extension",
              "version": "0.1.0",
              "runtimeAbiVersion": "0.1.0",
              "kind": "node-pack",
              "provides": {},
              "permissions": []
            }"#,
        )
        .unwrap();
    }

    fn codes(issues: &[PackageIssueV01]) -> Vec<&str> {
        issues.iter().map(|issue| issue.code.as_str()).collect()
    }

    fn assert_no_absolute_root(response: &PackageRegistryListResponseV01, root: &Path) {
        let serialized = serde_json::to_string(response).unwrap();
        assert!(
            !serialized.contains(&root.display().to_string()),
            "public package DTO leaked absolute root path: {serialized}"
        );
    }

    #[test]
    fn package_paths_parse_like_runtime_environment() {
        let first = PathBuf::from("/tmp/skenion-package-one");
        let second = PathBuf::from("/tmp/skenion-package-two");
        let joined = env::join_paths([&first, &second]).unwrap();

        let manager = RuntimePackageManager::from_package_paths(Some(joined));

        assert_eq!(manager.package_dirs, vec![first, second]);
    }

    #[test]
    fn valid_package_root_appears_in_registry_snapshot() {
        let package_dir = temp_dir("valid");
        write_package_manifest(&package_dir, &valid_manifest("example/package"));
        let manager = RuntimePackageManager::with_package_dirs(vec![package_dir.clone()]);

        let scan = manager.scan_registry();
        let response = scan.snapshot.response();

        assert!(response.ok);
        assert_eq!(response.packages.len(), 1);
        assert_eq!(response.packages[0].package_id, "example/package");
        assert_eq!(response.packages[0].version, "0.49.0");
        assert_eq!(
            response.packages[0].contracts.version,
            skenion_contracts::CONTRACTS_PACKAGE_VERSION
        );
        assert_eq!(response.packages[0].source, PackageSourceV01::Workspace);
        assert_eq!(response.packages[0].root, PackageRootKindV01::Package);
        assert_eq!(response.packages[0].trust, PackageTrustV01::Trusted);
        assert_eq!(
            response.packages[0].provides.patches[0].id,
            "example.package.main"
        );
        assert_eq!(
            response.packages[0].manifest_path,
            RUNTIME_PACKAGE_MANIFEST_FILE
        );
        assert_eq!(response.packages[0].manifest_checksum.value.len(), 64);
        assert_eq!(scan.snapshot.revision(), 1);
        assert_eq!(scan.snapshot.event_id(), "package-registry-event-000001");
        assert_eq!(scan.snapshot.state(), RuntimePackageRegistryState::Ready);
        assert_no_absolute_root(&response, &package_dir);
    }

    #[test]
    fn extension_only_root_fails_closed() {
        let package_dir = temp_dir("extension-only");
        write_extension_manifest(&package_dir);
        let manager = RuntimePackageManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_packages();

        assert!(!response.ok);
        assert!(response.packages.is_empty());
        assert_eq!(codes(&response.issues), vec!["package.root.extension-only"]);
    }

    #[test]
    fn both_manifest_root_fails_closed() {
        let package_dir = temp_dir("both-manifests");
        write_package_manifest(&package_dir, &valid_manifest("example/both"));
        write_extension_manifest(&package_dir);
        let manager = RuntimePackageManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_packages();

        assert!(!response.ok);
        assert!(response.packages.is_empty());
        assert_eq!(codes(&response.issues), vec!["package.root.both-manifests"]);
    }

    #[test]
    fn malformed_manifest_produces_structured_issue() {
        let package_dir = temp_dir("malformed");
        write_package_manifest(&package_dir, "{ not-json");
        let manager = RuntimePackageManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_packages();

        assert!(!response.ok);
        assert!(response.packages.is_empty());
        assert_eq!(
            codes(&response.issues),
            vec!["package.manifest.parse-failed"]
        );
        assert_eq!(
            response.issues[0]
                .details
                .as_ref()
                .unwrap()
                .get("manifestPath"),
            Some(&json!(RUNTIME_PACKAGE_MANIFEST_FILE))
        );
    }

    #[test]
    fn missing_and_unreadable_package_roots_are_structured_issues() {
        let missing_manifest_dir = temp_dir("missing-manifest");
        let unreadable_dir = temp_dir("unreadable-root");
        fs::remove_dir_all(&unreadable_dir).unwrap();
        let manager = RuntimePackageManager::with_package_dirs(vec![
            missing_manifest_dir.clone(),
            unreadable_dir.clone(),
        ]);

        let response = manager.list_packages();

        assert!(!response.ok);
        assert!(response.packages.is_empty());
        assert_eq!(
            codes(&response.issues),
            vec!["package.manifest.missing", "package.root.unreadable"]
        );
        assert_no_absolute_root(&response, &missing_manifest_dir);
        assert_no_absolute_root(&response, &unreadable_dir);
    }

    #[test]
    fn contract_invalid_manifest_stays_in_registry_with_package_issue() {
        let package_dir = temp_dir("contract-invalid");
        let body = valid_manifest("bad id").replace(
            &format!(
                r#""contracts": {{ "version": "{}" }}"#,
                skenion_contracts::CONTRACTS_PACKAGE_VERSION
            ),
            r#""contracts": { "version": "*" }"#,
        );
        write_package_manifest(&package_dir, &body);
        let manager = RuntimePackageManager::with_package_dirs(vec![package_dir.clone()]);

        let response = manager.list_packages();

        assert!(!response.ok);
        assert_eq!(response.packages.len(), 1);
        assert_eq!(response.packages[0].package_id, "bad id");
        assert!(codes(&response.packages[0].issues).contains(&"package.manifest.contract-invalid"));
        assert!(codes(&response.packages[0].issues).contains(&"package.manifest.contract-invalid"));
        assert_no_absolute_root(&response, &package_dir);
    }

    #[test]
    fn manifest_projection_fields_fail_closed() {
        let package_dir = temp_dir("legacy-projection-fields");
        let body = valid_manifest("example/legacy-fields").replace(
            "\"category\": \"patch\",",
            r#""category": "patch",
              "source": "workspace",
              "root": "package",
              "trust": "trusted","#,
        );
        write_package_manifest(&package_dir, &body);
        let manager = RuntimePackageManager::with_package_dirs(vec![package_dir]);

        let response = manager.list_packages();

        assert!(!response.ok);
        assert!(response.packages.is_empty());
        assert_eq!(
            codes(&response.issues),
            vec!["package.manifest.decode-failed"]
        );
    }

    #[test]
    fn absolute_parent_and_symlink_escape_paths_are_issues() {
        let package_dir = temp_dir("path-issues");
        let outside_dir = temp_dir("path-issues-outside");
        fs::write(outside_dir.join("secret.txt"), "secret").unwrap();
        symlink_file(
            &outside_dir.join("secret.txt"),
            &package_dir.join("linked-secret.txt"),
        )
        .unwrap();
        let body = valid_manifest("example/pathdiag")
            .replace("patches/main.skenion.json", "/tmp/absolute.skenion.json")
            .replace("evidence/manifest.sha256", "../outside.sha256")
            .replace("skenion.package.json", "linked-secret.txt");
        write_package_manifest(&package_dir, &body);
        let manager = RuntimePackageManager::with_package_dirs(vec![package_dir.clone()]);

        let response = manager.list_packages();

        assert!(!response.ok);
        assert_eq!(response.packages.len(), 1);
        let package_codes = codes(&response.packages[0].issues);
        assert!(package_codes.contains(&"package.path.invalid"));
        assert!(package_codes.contains(&"package.path.symlink-escape"));
        assert_no_absolute_root(&response, &package_dir);
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("/tmp/absolute")
        );
    }

    #[test]
    fn every_manifest_declared_path_surface_is_checked_and_public_paths_are_sanitized() {
        let package_dir = temp_dir("all-path-surfaces");
        let contracts_version = skenion_contracts::CONTRACTS_PACKAGE_VERSION;
        let body = format!(
            r#"{{
          "schema": "skenion.package.manifest",
          "schemaVersion": "0.1.0",
          "id": "example/all-paths",
          "version": "0.49.0",
          "category": "mixed",
          "contracts": {{ "version": "{contracts_version}" }},
          "runtimeAbiRange": ">=0.1.0 <0.2.0",
          "targets": ["x86_64-unknown-linux-gnu"],
          "provides": {{
            "patches": [{{ "id": "example.all.patch", "path": "/absolute/patch.skenion.json" }}],
            "nodes": [{{ "id": "example.all.node", "path": "../nodes/node.json" }}],
            "resources": [{{ "id": "example.all.resource", "path": "" }}],
            "help": [{{ "id": "example.all.help", "path": "../help/help.skenion.json" }}]
          }},
          "paths": {{
            "patches": ["../patches"],
            "resources": ["/absolute/resources"],
            "docs": [""],
            "tests": ["../tests"]
          }},
          "checksums": [
            {{
              "id": "manifest",
              "path": "/absolute/checksum.sha256",
              "checksum": {{
                "algorithm": "sha256",
                "value": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
              }}
            }}
          ],
          "evidence": [
            {{
              "id": "sbom",
              "kind": "sbom",
              "path": "../evidence/sbom.json",
              "checksum": {{
                "algorithm": "sha256",
                "value": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
              }}
            }}
          ],
          "nativeArtifacts": [
            {{
              "target": "x86_64-unknown-linux-gnu",
              "path": "/absolute/libexample.so",
              "entrypoint": "skenion_extension_init",
              "checksum": {{
                "algorithm": "sha256",
                "value": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
              }},
              "evidenceRefs": ["sbom"]
            }}
          ]
        }}"#
        );
        write_package_manifest(&package_dir, &body);
        let manager = RuntimePackageManager::with_package_dirs(vec![package_dir.clone()]);

        let response = manager.list_packages();

        assert!(!response.ok);
        assert_eq!(response.packages.len(), 1);
        assert_eq!(
            response.packages[0].provides.patches[0].path,
            "<redacted:absolute-path>"
        );
        assert_eq!(
            response.packages[0].provides.nodes[0].path,
            "../nodes/node.json"
        );
        assert_eq!(response.packages[0].provides.resources[0].path, "");
        assert_eq!(
            response.packages[0].provides.help[0].path,
            "../help/help.skenion.json"
        );
        let path_issues = response.packages[0]
            .issues
            .iter()
            .filter(|issue| issue.code == "package.path.invalid")
            .collect::<Vec<_>>();
        assert!(path_issues.len() >= 11);
        assert!(path_issues.iter().any(|issue| {
            issue
                .details
                .as_ref()
                .and_then(|details| details.get("pathKind"))
                == Some(&json!("native-artifact-path"))
        }));
        assert_no_absolute_root(&response, &package_dir);
    }

    #[test]
    fn duplicate_and_overlapping_roots_are_issues() {
        let parent = temp_dir("overlap-parent");
        let child = parent.join("child");
        fs::create_dir_all(&child).unwrap();
        write_package_manifest(&parent, &valid_manifest("example/parent"));
        write_package_manifest(&child, &valid_manifest("example/child"));
        let manager =
            RuntimePackageManager::with_package_dirs(vec![parent.clone(), parent.clone(), child]);

        let response = manager.list_packages();

        assert!(!response.ok);
        let top_level_codes = codes(&response.issues);
        assert!(top_level_codes.contains(&"package.root.duplicate"));
        assert!(top_level_codes.contains(&"package.root.overlap"));
    }

    #[test]
    fn duplicate_package_identity_is_issue() {
        let first = temp_dir("duplicate-package-one");
        let second = temp_dir("duplicate-package-two");
        write_package_manifest(&first, &valid_manifest("example/duplicate"));
        write_package_manifest(&second, &valid_manifest("example/duplicate"));
        let manager = RuntimePackageManager::with_package_dirs(vec![first, second]);

        let response = manager.list_packages();

        assert!(!response.ok);
        assert!(codes(&response.issues).contains(&"package.registry.duplicate-package"));
    }

    #[test]
    fn registry_private_helpers_keep_public_dtos_quiet_and_redacted() {
        let mut provides = PackageProvidesV01 {
            patches: vec![provided_ref(
                "example.patch",
                "/absolute/patch.skenion.json",
            )],
            nodes: vec![provided_ref("example.node", "nodes/node.json")],
            objects: Vec::new(),
            resources: vec![provided_ref("example.resource", "/absolute/resource.bin")],
            help: vec![provided_ref("example.help", "help/help.skenion.json")],
        };

        provides = public_package_provides(provides);

        assert_eq!(provides.patches[0].path, "<redacted:absolute-path>");
        assert_eq!(provides.nodes[0].path, "nodes/node.json");
        assert_eq!(provides.resources[0].path, "<redacted:absolute-path>");
        assert_eq!(provides.help[0].path, "help/help.skenion.json");
        assert_eq!(package_path_violation(""), "empty");
        assert_eq!(package_path_violation("/absolute"), "absolute");
        assert_eq!(package_path_violation("../parent"), "parent-directory");
        assert_eq!(package_path_violation("relative"), "unknown");
        assert_eq!(
            package_root_display_name(3, Path::new("/")),
            "package-root-3"
        );

        let info = PackageRootInfo {
            index: 3,
            path: PathBuf::from("/redacted/root"),
            canonical_path: None,
            display_name: "root-name".to_owned(),
        };
        let root_diag = root_issue(
            PackageIssueSeverityV01::Warning,
            "package.root.warning",
            "root warning",
            &info,
            Some(RUNTIME_PACKAGE_MANIFEST_FILE),
            json!("ignored-non-object"),
        );
        let package_diag = package_issue(
            PackageIssueSeverityV01::Info,
            "package.info",
            "package info",
            registry_issue_details(
                "package-registry-entry",
                Some(RUNTIME_PACKAGE_MANIFEST_FILE),
                Some("example/package"),
                Some("0.49.0"),
                json!("ignored-non-object"),
            ),
        );
        let package_error = package_issue(
            PackageIssueSeverityV01::Error,
            "package.error",
            "package error",
            registry_issue_details(
                "package-registry-entry",
                Some(RUNTIME_PACKAGE_MANIFEST_FILE),
                Some("example/package"),
                Some("0.49.0"),
                json!({ "reason": "test" }),
            ),
        );
        let response = PackageRegistryListResponseV01 {
            ok: false,
            packages: vec![PackageRegistryEntryV01 {
                package_id: "example/package".to_owned(),
                version: "0.49.0".to_owned(),
                category: skenion_contracts::PackageCategoryV01::Patch,
                source: PackageSourceV01::Workspace,
                root: PackageRootKindV01::Package,
                trust: PackageTrustV01::Trusted,
                contracts: PackageContractsRequirementV01 {
                    version: skenion_contracts::CONTRACTS_PACKAGE_VERSION.to_owned(),
                },
                runtime_abi_range: None,
                targets: Vec::new(),
                manifest_path: RUNTIME_PACKAGE_MANIFEST_FILE.to_owned(),
                root_path: None,
                manifest_checksum: PackageChecksumV01 {
                    algorithm: PackageChecksumAlgorithmV01::Sha256,
                    value: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_owned(),
                },
                provides,
                issues: vec![package_diag, package_error],
            }],
            issues: vec![root_diag],
        };

        assert!(has_error(&response));
        assert_eq!(
            response.issues[0].severity,
            PackageIssueSeverityV01::Warning
        );
        assert_eq!(response.issues[0].code, "package.root.warning");
        assert_eq!(
            response.packages[0].issues[1].severity,
            PackageIssueSeverityV01::Error
        );
        assert_eq!(response.packages[0].issues[1].code, "package.error");
    }

    fn provided_ref(id: &str, path: &str) -> PackageProvidedRefV01 {
        PackageProvidedRefV01 {
            id: id.to_owned(),
            path: path.to_owned(),
            description: Some("provided item".to_owned()),
        }
    }

    #[cfg(unix)]
    fn symlink_file(original: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }

    #[cfg(windows)]
    fn symlink_file(original: &Path, link: &Path) -> io::Result<()> {
        std::os::windows::fs::symlink_file(original, link)
    }
}
