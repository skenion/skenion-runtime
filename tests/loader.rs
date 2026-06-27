use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use skenion_runtime::{
    NodeDefinitionCurrent, ProjectRequestCurrent, build_execution_plan_request_current,
    validate_project_request_current,
};

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

fn read_json_file<T: DeserializeOwned>(path: impl AsRef<Path>) -> T {
    let path = path.as_ref();
    let bytes = fs::read(path)
        .unwrap_or_else(|error| panic!("expected JSON fixture at {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("expected {} to parse: {error}", path.display()))
}

fn valid_definition_current() -> Value {
    json!({
      "schema": "skenion.node.definition",
      "schemaVersion": "0.1.0",
      "id": "object.core.loader",
      "version": "0.1.0",
      "displayName": "Loader",
      "category": "Core",
      "ports": [
        {
          "id": "out",
          "direction": "output",
          "type": "value.core.float32",
          "rate": "control"
        }
      ],
      "execution": { "model": "control" },
      "state": { "persistent": false },
      "permissions": [],
      "capabilities": []
    })
}

fn valid_project_request_current() -> Value {
    json!({
      "graph": {
        "schema": "skenion.graph",
        "schemaVersion": "0.1.0",
        "id": "loader-graph-current",
        "revision": "1",
        "nodes": [
          {
            "id": "node",
            "kind": "object.core.loader",
            "kindVersion": "0.1.0",
            "params": {},
            "ports": [
              {
                "id": "out",
                "direction": "output",
                "type": "value.core.float32",
                "rate": "control"
              }
            ]
          }
        ],
        "edges": []
      },
      "nodes": [valid_definition_current()]
    })
}

#[test]
fn loads_current_current_node_definition_and_project_request() {
    let temp = TempDir::new("valid");
    let definition_path = temp.file("node.json");
    let project_path = temp.file("project.json");
    fs::write(&definition_path, valid_definition_current().to_string()).unwrap();
    fs::write(&project_path, valid_project_request_current().to_string()).unwrap();

    let definition: NodeDefinitionCurrent = read_json_file(&definition_path);
    let request: ProjectRequestCurrent = read_json_file(&project_path);

    assert_eq!(definition.schema_version, "0.1.0");
    assert_eq!(definition.id, "object.core.loader");
    assert_eq!(request.graph.schema_version, "0.1.0");
    assert_eq!(request.graph.id, "loader-graph-current");
    validate_project_request_current(&request)
        .expect("current 0.1 project request should validate");
}

#[test]
fn rejects_unsupported_project_versions_under_current_surface() {
    let mut payload = valid_project_request_current();
    payload["graph"]["schemaVersion"] = json!("9.9.9");
    let request: ProjectRequestCurrent =
        serde_json::from_value(payload).expect("payload shape should still deserialize");

    let diagnostics = validate_project_request_current(&request)
        .expect_err("active Runtime project validation must reject non-current graph versions");
    assert_eq!(
        diagnostics[0].code.as_deref(),
        Some("graph.invalid-contract")
    );
    assert_eq!(diagnostics[0].details.as_ref().unwrap()["surface"], "graph");
    assert_eq!(
        diagnostics[0].details.as_ref().unwrap()["expectedSchemaVersion"],
        "0.1.0"
    );
    assert_eq!(
        diagnostics[0].details.as_ref().unwrap()["receivedSchemaVersion"],
        "9.9.9"
    );
}

#[test]
fn current_project_request_plans_value_number_graph() {
    let project: ProjectRequestCurrent = serde_json::from_value(valid_project_request_current())
        .expect("current project request should parse");

    let source_node = project
        .graph
        .nodes
        .first()
        .expect("graph should include node");
    let source_port = source_node
        .ports
        .iter()
        .find(|port| port.id == "out")
        .expect("source should expose value output");

    assert_eq!(project.graph.schema_version, "0.1.0");
    assert_eq!(source_port.port_type, "value.core.float32");

    validate_project_request_current(&project)
        .expect("canonical current 0.1 project should validate");
    let (plan, diagnostics) = build_execution_plan_request_current(&project)
        .expect("canonical current 0.1 project should plan");

    assert!(diagnostics.is_empty());
    assert_eq!(plan.graph_id(), "loader-graph-current");
    assert!(plan.contains_node("node"));
}
