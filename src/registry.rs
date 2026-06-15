use std::{
    collections::HashMap,
    fs,
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
        let entry = entry.map_err(|source| RegistryLoadError::ReadDir {
            path: path.display().to_string(),
            source,
        })?;
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
