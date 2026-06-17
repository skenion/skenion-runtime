use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{NodeDefinition, ValidationReport, validate_node_definition};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeDefinitionKey {
    pub id: String,
    pub version: String,
}

impl NodeDefinitionKey {
    pub fn new(id: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct NodeRegistry {
    definitions: HashMap<NodeDefinitionKey, NodeDefinition>,
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("duplicate node definition: {id}@{version}")]
    DuplicateDefinition { id: String, version: String },
    #[error("invalid node definition {id}@{version}: {source}")]
    InvalidDefinition {
        id: String,
        version: String,
        #[source]
        source: ValidationReport,
    },
}

#[derive(Debug, Error)]
pub enum RegistryLoadError {
    #[error("failed to read directory {path}: {source}")]
    ReadDir {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid {path}: {source}")]
    Invalid {
        path: String,
        #[source]
        source: RegistryError,
    },
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load_dir(path: impl AsRef<Path>) -> Result<Self, RegistryLoadError> {
        let mut registry = Self::new();
        let mut files = Vec::new();
        collect_json_files(path.as_ref(), &mut files)?;
        files.sort();

        for file in files {
            let display_path = file.display().to_string();
            let bytes = fs::read(&file).map_err(|source| RegistryLoadError::ReadFile {
                path: display_path.clone(),
                source,
            })?;
            let definition: NodeDefinition =
                serde_json::from_slice(&bytes).map_err(|source| RegistryLoadError::Parse {
                    path: display_path.clone(),
                    source,
                })?;
            registry
                .insert(definition)
                .map_err(|source| RegistryLoadError::Invalid {
                    path: display_path,
                    source,
                })?;
        }

        Ok(registry)
    }

    pub fn insert(&mut self, definition: NodeDefinition) -> Result<(), RegistryError> {
        validate_node_definition(&definition).map_err(|source| {
            RegistryError::InvalidDefinition {
                id: definition.id.clone(),
                version: definition.version.clone(),
                source,
            }
        })?;

        let key = NodeDefinitionKey::new(definition.id.clone(), definition.version.clone());
        if self.definitions.contains_key(&key) {
            return Err(RegistryError::DuplicateDefinition {
                id: key.id,
                version: key.version,
            });
        }

        self.definitions.insert(key, definition);
        Ok(())
    }

    pub fn get(&self, id: &str, version: &str) -> Option<&NodeDefinition> {
        self.definitions
            .get(&NodeDefinitionKey::new(id.to_owned(), version.to_owned()))
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

fn collect_json_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), RegistryLoadError> {
    let entries = fs::read_dir(path).map_err(|source| RegistryLoadError::ReadDir {
        path: path.display().to_string(),
        source,
    })?;

    for entry in entries {
        let entry = read_dir_entry(path, entry)?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            collect_json_files(&entry_path, files)?;
        } else if entry_path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            files.push(entry_path);
        }
    }

    Ok(())
}

fn read_dir_entry(
    path: &Path,
    entry: io::Result<fs::DirEntry>,
) -> Result<fs::DirEntry, RegistryLoadError> {
    entry.map_err(|source| RegistryLoadError::ReadDir {
        path: path.display().to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use serde_json::{Value, json};

    use super::*;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let id = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "skenion-runtime-registry-{name}-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp dir should be created");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn definition_value(id: &str) -> Value {
        json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": id,
          "version": "0.1.0",
          "displayName": "Node",
          "category": "Core",
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "value", "dataKind": "number.f32" } }
          ],
          "execution": { "model": "value" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        })
    }

    fn definition(id: &str) -> NodeDefinition {
        serde_json::from_value(definition_value(id)).expect("definition should deserialize")
    }

    #[test]
    fn key_new_and_basic_registry_methods_work() {
        let key = NodeDefinitionKey::new("core.node", "0.1.0");
        assert_eq!(key.id, "core.node");
        assert_eq!(key.version, "0.1.0");

        let mut registry = NodeRegistry::new();
        assert!(registry.is_empty());
        registry.insert(definition("core.node")).unwrap();

        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
        assert!(registry.get("core.node", "0.1.0").is_some());
        assert!(registry.get("core.node", "0.2.0").is_none());
    }

    #[test]
    fn insert_rejects_duplicate_and_invalid_definitions() {
        let mut registry = NodeRegistry::new();
        registry.insert(definition("core.node")).unwrap();

        let duplicate = registry.insert(definition("core.node")).unwrap_err();
        assert_eq!(
            duplicate.to_string(),
            "duplicate node definition: core.node@0.1.0"
        );

        let invalid: NodeDefinition = serde_json::from_value(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "9.9.9",
          "id": "core.invalid",
          "version": "0.1.0",
          "displayName": "Invalid",
          "category": "Core",
          "ports": [],
          "execution": { "model": "value" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
        .unwrap();
        let invalid_error = registry.insert(invalid).unwrap_err();
        assert!(
            invalid_error
                .to_string()
                .contains("invalid node definition core.invalid@0.1.0")
        );
    }

    #[test]
    fn load_dir_recursively_loads_json_files_in_sorted_order() {
        let temp = TempDir::new("recursive");
        let nested = temp.path().join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            temp.path().join("b.json"),
            definition_value("core.b").to_string(),
        )
        .unwrap();
        fs::write(
            nested.join("a.json"),
            definition_value("core.a").to_string(),
        )
        .unwrap();
        fs::write(temp.path().join("ignored.txt"), "not json").unwrap();

        let registry = NodeRegistry::load_dir(temp.path()).unwrap();

        assert_eq!(registry.len(), 2);
        assert!(registry.get("core.a", "0.1.0").is_some());
        assert!(registry.get("core.b", "0.1.0").is_some());
    }

    #[test]
    fn load_dir_reports_read_dir_parse_and_invalid_errors() {
        let missing = NodeRegistry::load_dir("/definitely/missing/skenion-runtime-registry");
        assert!(matches!(missing, Err(RegistryLoadError::ReadDir { .. })));

        let parse_temp = TempDir::new("parse");
        fs::write(parse_temp.path().join("bad.json"), "{").unwrap();
        let parse = NodeRegistry::load_dir(parse_temp.path());
        assert!(matches!(parse, Err(RegistryLoadError::Parse { .. })));

        let invalid_temp = TempDir::new("invalid");
        let mut invalid = definition_value("core.invalid");
        invalid["schemaVersion"] = json!("9.9.9");
        fs::write(
            invalid_temp.path().join("invalid.json"),
            invalid.to_string(),
        )
        .unwrap();
        let invalid = NodeRegistry::load_dir(invalid_temp.path());
        assert!(matches!(invalid, Err(RegistryLoadError::Invalid { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn load_dir_reports_read_file_errors() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new("read-file");
        symlink(
            temp.path().join("missing-target.json"),
            temp.path().join("broken.json"),
        )
        .unwrap();

        let error = NodeRegistry::load_dir(temp.path());
        assert!(matches!(error, Err(RegistryLoadError::ReadFile { .. })));
    }

    #[test]
    fn read_dir_entry_reports_iteration_errors() {
        let error = read_dir_entry(
            Path::new("/tmp/skenion"),
            Err(io::Error::other("iteration failed")),
        )
        .unwrap_err();

        assert!(matches!(error, RegistryLoadError::ReadDir { .. }));
        assert!(error.to_string().contains("/tmp/skenion"));
    }
}
