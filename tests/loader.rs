use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};
use skenion_runtime::{LoadError, load_graph_document, load_node_definition};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "skenion-runtime-loader-{name}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir should be created");
        Self { path }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn valid_definition() -> Value {
    json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "core.loader",
      "version": "0.1.0",
      "displayName": "Loader",
      "category": "Core",
      "ports": [
        { "id": "out", "direction": "output", "type": { "flow": "value", "dataKind": "f32" } }
      ],
      "execution": { "model": "value" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    })
}

fn valid_graph() -> Value {
    json!({
      "schema": "skenion.graph",
      "schemaVersion": "0.1.0",
      "id": "loader-graph",
      "revision": "1",
      "nodes": [
        {
          "id": "node",
          "kind": "core.loader",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "value", "dataKind": "f32" } }
          ]
        }
      ],
      "edges": []
    })
}

fn error_kind(error: LoadError) -> &'static str {
    match error {
        LoadError::Read { .. } => "read",
        LoadError::Parse { .. } => "parse",
        LoadError::Invalid { .. } => "invalid",
    }
}

#[test]
fn loads_valid_node_definition_and_graph() {
    let temp = TempDir::new("valid");
    let definition_path = temp.file("node.json");
    let graph_path = temp.file("graph.json");
    fs::write(&definition_path, valid_definition().to_string()).unwrap();
    fs::write(&graph_path, valid_graph().to_string()).unwrap();

    let definition = load_node_definition(&definition_path).unwrap();
    let graph = load_graph_document(&graph_path).unwrap();

    assert_eq!(definition.id, "core.loader");
    assert_eq!(graph.id, "loader-graph");
}

#[test]
fn reports_read_parse_and_invalid_errors_for_node_definitions() {
    let temp = TempDir::new("node-errors");
    let missing = load_node_definition(temp.file("missing.json"));
    assert_eq!(error_kind(missing.unwrap_err()), "read");

    let parse_path = temp.file("parse.json");
    fs::write(&parse_path, "{").unwrap();
    assert_eq!(
        error_kind(load_node_definition(&parse_path).unwrap_err()),
        "parse"
    );

    let invalid_path = temp.file("invalid.json");
    let mut invalid = valid_definition();
    invalid["schemaVersion"] = json!("9.9.9");
    fs::write(&invalid_path, invalid.to_string()).unwrap();
    assert_eq!(
        error_kind(load_node_definition(&invalid_path).unwrap_err()),
        "invalid"
    );
}

#[test]
fn reports_read_parse_and_invalid_errors_for_graphs() {
    let temp = TempDir::new("graph-errors");
    let missing = load_graph_document(temp.file("missing.json"));
    assert_eq!(error_kind(missing.unwrap_err()), "read");

    let parse_path = temp.file("parse.json");
    fs::write(&parse_path, "{").unwrap();
    assert_eq!(
        error_kind(load_graph_document(&parse_path).unwrap_err()),
        "parse"
    );

    let invalid_path = temp.file("invalid.json");
    let mut invalid = valid_graph();
    invalid["schemaVersion"] = json!("9.9.9");
    fs::write(&invalid_path, invalid.to_string()).unwrap();
    assert_eq!(
        error_kind(load_graph_document(&invalid_path).unwrap_err()),
        "invalid"
    );
}
