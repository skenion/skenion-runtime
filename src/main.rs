use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use skenion_runtime::{
    DEFAULT_HOST, DEFAULT_PORT, ExecutionPlan, NodeDefinitionCurrent, PreviewFrameLimit,
    ProjectDocumentCurrent, ProjectRequestCurrent, RunProjectRequestCurrent, RuntimeDiagnostic,
    ServeRuntimeOptions, build_execution_plan_request_current,
    build_execution_plan_run_request_current, format_dummy_execution_text,
    format_midi_clock_fixture_report_text, format_plan_text,
    project_document_payload_schema_diagnostics, project_document_validation_diagnostics_current,
    run_dummy_execution, run_midi_clock_fixture_file, run_preview_window,
    run_render_preview_document_file, schema_version_diagnostic, serve_runtime,
    serve_runtime_with_options, validate_project_request_current,
};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate an active ProjectDocumentCurrent or current 0.1 project request file.
    ValidateProject {
        /// Path to the current 0.1 project JSON file.
        #[arg(long)]
        project: PathBuf,
    },
    /// Build an execution plan skeleton for an active current 0.1 project file.
    Plan {
        /// Path to the current 0.1 project JSON file.
        #[arg(long)]
        project: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Run a deterministic dummy execution from an execution plan.
    Run {
        /// Path to the current 0.1 project JSON file.
        #[arg(long)]
        project: PathBuf,
        /// Number of dummy frames to simulate.
        #[arg(long, default_value_t = 1)]
        frames: usize,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Open a local placeholder preview window from a prepared execution plan.
    PreviewPlan {
        /// Path to the prepared execution plan JSON.
        #[arg(long)]
        plan: PathBuf,
        /// Keep the preview open until the window is closed.
        #[arg(long)]
        until_close: bool,
        /// Number of placeholder frames before the preview exits.
        #[arg(long, default_value_t = 300)]
        frames: usize,
    },
    /// Open a local render preview window from a prepared preview document.
    PreviewDocument {
        /// Path to the prepared preview document JSON.
        #[arg(long)]
        document: PathBuf,
        /// Path where the preview child should write render telemetry heartbeat JSON.
        #[arg(long)]
        telemetry: Option<PathBuf>,
        /// Path where the preview child should read live runtime control state snapshots.
        #[arg(long)]
        control_state: Option<PathBuf>,
        /// Keep the preview open until the window is closed.
        #[arg(long)]
        until_close: bool,
        /// Number of frames before the preview exits.
        #[arg(long, default_value_t = 300)]
        frames: usize,
    },
    /// Run the Runtime MIDI Clock fixture parser.
    ClockMidi {
        /// Path to a simulated MIDI Clock fixture.
        #[arg(long)]
        simulate: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Start the local HTTP JSON control API.
    Serve {
        /// Host to bind. Defaults to localhost for local development safety.
        #[arg(long, default_value = DEFAULT_HOST)]
        host: String,
        /// Port to bind.
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// Print machine-readable local-managed sidecar startup JSON after binding.
        #[arg(long)]
        startup_json: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Err(error) = run(cli).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::ValidateProject { project } => {
            let request = load_project_request_current(project)?;
            if let Err(diagnostics) = validate_project_request_current(&request) {
                return Err(format_runtime_diagnostics(&diagnostics).into());
            }
            println!(
                "valid project: {} {}",
                request.graph.id, request.graph.revision
            );
            Ok(())
        }
        Command::Plan { project, format } => {
            let request = load_project_request_current(project)?;
            let (plan, diagnostics) = build_execution_plan_request_current(&request)
                .map_err(|diagnostics| format_runtime_diagnostics(&diagnostics))?;
            if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.is_some())
            {
                eprintln!("{}", format_runtime_diagnostics(&diagnostics));
            }
            match format {
                OutputFormat::Text => {
                    print!("{}", format_plan_text(&plan));
                }
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&plan)?);
                }
            }
            Ok(())
        }
        Command::Run {
            project,
            frames,
            format,
        } => {
            let request = load_run_project_request_current(project, frames)?;
            let (plan, diagnostics) = build_execution_plan_run_request_current(&request)
                .map_err(|diagnostics| format_runtime_diagnostics(&diagnostics))?;
            if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.is_some())
            {
                eprintln!("{}", format_runtime_diagnostics(&diagnostics));
            }
            let report = run_dummy_execution(&plan, frames);
            match format {
                OutputFormat::Text => {
                    print!("{}", format_dummy_execution_text(&report));
                }
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
            }
            Ok(())
        }
        Command::PreviewPlan {
            plan,
            until_close,
            frames,
        } => {
            let plan = load_execution_plan(plan)?;
            let frame_limit = if until_close {
                PreviewFrameLimit::UntilClose
            } else {
                PreviewFrameLimit::Frames(frames)
            };
            run_preview_window(plan, frame_limit)
        }
        Command::PreviewDocument {
            document,
            telemetry,
            control_state,
            until_close,
            frames,
        } => {
            let frame_limit = if until_close {
                PreviewFrameLimit::UntilClose
            } else {
                PreviewFrameLimit::Frames(frames)
            };
            run_render_preview_document_file(document, frame_limit, telemetry, control_state)
        }
        Command::ClockMidi { simulate, format } => run_clock_midi(simulate, format),
        Command::Serve {
            host,
            port,
            startup_json,
        } => {
            if startup_json {
                serve_runtime_with_options(&host, port, ServeRuntimeOptions { startup_json }).await
            } else {
                serve_runtime(&host, port).await
            }
        }
    }
}

fn load_project_request_current(
    path: PathBuf,
) -> Result<ProjectRequestCurrent, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if is_project_document(&value) {
        validate_project_document_schema_version(&value).map_err(diagnostics_error)?;
        decode_project_document_request_current(value)
    } else {
        let request: ProjectRequestCurrent = serde_json::from_value(value)?;
        if request.graph.schema_version != "0.1.0" {
            let diagnostic =
                schema_version_diagnostic("graph", Some(request.graph.schema_version.as_str()))
                    .expect("non-current schema version should produce a diagnostic");
            return Err(diagnostics_error(vec![diagnostic]));
        }
        Ok(request)
    }
}

fn load_run_project_request_current(
    path: PathBuf,
    frames: usize,
) -> Result<RunProjectRequestCurrent, Box<dyn std::error::Error>> {
    let request = load_project_request_current(path)?;
    Ok(RunProjectRequestCurrent {
        document: request.document,
        graph: request.graph,
        nodes: request.nodes,
        patch_library: request.patch_library,
        view_state: request.view_state,
        frames: Some(frames),
    })
}

fn is_project_document(value: &serde_json::Value) -> bool {
    value
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|schema| schema == "skenion.project")
}

fn validate_project_document_schema_version(
    value: &serde_json::Value,
) -> Result<(), Vec<RuntimeDiagnostic>> {
    let diagnostic = schema_version_diagnostic(
        "project",
        value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_str),
    );
    match diagnostic {
        Some(diagnostic) => Err(vec![diagnostic]),
        None => Ok(()),
    }
}

fn decode_project_document_request_current(
    mut value: serde_json::Value,
) -> Result<ProjectRequestCurrent, Box<dyn std::error::Error>> {
    let schema_diagnostics = project_document_payload_schema_diagnostics(&value);
    if !schema_diagnostics.is_empty() {
        return Err(diagnostics_error(schema_diagnostics));
    }
    let nodes = value
        .as_object_mut()
        .and_then(|object| object.remove("nodes"))
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    let nodes = serde_json::from_value::<Vec<NodeDefinitionCurrent>>(nodes)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("frames");
    }
    let document = serde_json::from_value::<ProjectDocumentCurrent>(value)?;
    if let Err(report) = skenion_contracts::validate_project_document_v01(&document) {
        return Err(diagnostics_error(
            project_document_validation_diagnostics_current(&document, &report),
        ));
    }
    Ok(ProjectRequestCurrent::from_project_document(
        document, nodes,
    ))
}

fn diagnostics_error(diagnostics: Vec<RuntimeDiagnostic>) -> Box<dyn std::error::Error> {
    format_runtime_diagnostics(&diagnostics).into()
}

fn format_runtime_diagnostics(diagnostics: &[RuntimeDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| match &diagnostic.code {
            Some(code) => {
                let mut line = format!("{code}: {}", diagnostic.message);
                if let Some(details) = &diagnostic.details {
                    line.push_str(" details=");
                    line.push_str(&details.to_string());
                }
                line
            }
            None => diagnostic.message.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_clock_midi(
    simulate: Option<PathBuf>,
    format: OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(simulate) = simulate else {
        return Err("clock-midi requires --simulate <path>".into());
    };
    let report = run_midi_clock_fixture_file(&simulate)?;
    match format {
        OutputFormat::Text => {
            print!("{}", format_midi_clock_fixture_report_text(&report));
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}

fn load_execution_plan(path: PathBuf) -> Result<ExecutionPlan, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use skenion_runtime::ExecutionPlan;

    #[test]
    fn binary_target_loads_execution_plan_without_exposing_plan_nodes() {
        let path = std::env::temp_dir().join(format!(
            "skenion-runtime-cli-plan-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            json!({
              "graphId": "render-graph",
              "graphRevision": "1",
              "nodes": [
                {
                  "nodeId": "clear_1",
                  "kind": "object.core.render.clear-color",
                  "kindVersion": "0.1.0",
                  "executionModel": "gpu_pass",
                  "order": 0
                }
              ],
              "edges": [],
              "groups": []
            })
            .to_string(),
        )
        .expect("plan should write");
        let decoded: ExecutionPlan =
            super::load_execution_plan(path.clone()).expect("plan should load");

        assert_eq!(
            serde_json::to_value(decoded).expect("plan should serialize")["graphId"],
            "render-graph"
        );
        std::fs::remove_file(path).expect("test plan should be removable");
    }

    #[test]
    fn cli_project_loader_reports_structured_schema_version_diagnostics() {
        let path = std::env::temp_dir().join(format!(
            "skenion-runtime-cli-schema-version-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            json!({
              "graph": {
                "schema": "skenion.graph",
                "schemaVersion": "9.9.9",
                "id": "unsupported-cli-graph",
                "revision": "1",
                "nodes": [],
                "edges": []
              },
              "nodes": []
            })
            .to_string(),
        )
        .expect("test project request should be written");

        let error = super::load_project_request_current(path.clone())
            .expect_err("unsupported graph schema should be rejected")
            .to_string();
        std::fs::remove_file(path).expect("test project request should be removable");

        assert!(error.contains("project.unsupported-schema-version"));
        assert!(error.contains("\"surface\":\"graph\""));
        assert!(error.contains("\"expectedSchemaVersion\":\"0.1.0\""));
        assert!(error.contains("\"receivedSchemaVersion\":\"9.9.9\""));
    }
}
