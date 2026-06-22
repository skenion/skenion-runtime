use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use skenion_runtime::{
    AudioBackendConfig, AudioDspPlan, AudioDspPlanOptions, DEFAULT_HOST, DEFAULT_PORT,
    ExecutionPlan, NodeDefinitionV02, NodeRegistry, PreviewDocument, PreviewFrameLimit,
    ProjectDocumentV02, ProjectRequestV02, RunProjectRequestV02, RuntimeDiagnostic,
    ServeRuntimeOptions, build_audio_dsp_plan, build_execution_plan,
    build_execution_plan_request_v02, build_execution_plan_run_request_v02,
    format_dummy_execution_text, format_midi_clock_fixture_report_text, format_plan_text,
    load_graph_document, load_node_definition, run_dummy_execution, run_midi_clock_fixture_file,
    run_preview_window, run_render_preview_window, serve_runtime, serve_runtime_with_options,
    start_default_audio_output_backend, validate_project, validate_project_request_v02,
};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate a legacy Skenion Node Definition Manifest v0.1 JSON file.
    LegacyValidateNode {
        /// Path to the node definition manifest.
        path: PathBuf,
    },
    /// Validate a legacy Skenion Graph Document v0.1 JSON file.
    LegacyValidateGraph {
        /// Path to the graph document.
        path: PathBuf,
    },
    /// Validate a legacy graph v0.1 document against a node definition registry.
    LegacyValidateProject {
        /// Path to the graph document.
        #[arg(long)]
        graph: PathBuf,
        /// Directory containing node definition manifests.
        #[arg(long)]
        nodes: PathBuf,
    },
    /// Validate an active ProjectDocumentV02 or v0.2 project request file.
    ValidateProject {
        /// Path to the v0.2 project JSON file.
        #[arg(long)]
        project: PathBuf,
    },
    /// Build an execution plan skeleton for an active v0.2 project file.
    Plan {
        /// Path to the v0.2 project JSON file.
        #[arg(long)]
        project: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Build a legacy v0.1 audio DSP plan with endpoint, clock-domain, and bridge metadata.
    LegacyAudioPlan {
        /// Path to the graph document.
        #[arg(long)]
        graph: PathBuf,
        /// Directory containing node definition manifests.
        #[arg(long)]
        nodes: PathBuf,
        /// Internal DSP block size used by the audio plan.
        #[arg(long, default_value_t = 64)]
        block_size: u32,
        /// Sample rate used for unresolved planning metadata.
        #[arg(long, default_value_t = 48_000)]
        sample_rate: u32,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Run a deterministic dummy execution from an execution plan.
    Run {
        /// Path to the v0.2 project JSON file.
        #[arg(long)]
        project: PathBuf,
        /// Number of dummy frames to simulate.
        #[arg(long, default_value_t = 1)]
        frames: usize,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Open a local placeholder preview window from a legacy v0.1 graph.
    LegacyPreview {
        /// Path to the graph document.
        #[arg(long)]
        graph: PathBuf,
        /// Directory containing node definition manifests.
        #[arg(long)]
        nodes: PathBuf,
        /// Number of placeholder frames before the preview exits.
        #[arg(long, default_value_t = 300)]
        frames: usize,
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
    /// Run the CPAL default output backend for a legacy v0.1 audio.output DSP graph.
    LegacyAudioOutput {
        /// Path to the graph document.
        #[arg(long)]
        graph: PathBuf,
        /// Directory containing node definition manifests.
        #[arg(long)]
        nodes: PathBuf,
        /// Internal DSP block size used by the realtime executor.
        #[arg(long, default_value_t = 64)]
        block_size: u32,
        /// How long to keep the output backend alive.
        #[arg(long, default_value_t = 1000)]
        duration_ms: u64,
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
        Command::LegacyValidateNode { path } => load_node_definition(&path)
            .map(|definition| {
                println!(
                    "valid legacy node definition: {} {}",
                    definition.id, definition.version
                );
            })
            .map_err(Into::into),
        Command::LegacyValidateGraph { path } => load_graph_document(&path)
            .map(|graph| {
                println!("valid legacy graph: {} {}", graph.id, graph.revision);
            })
            .map_err(Into::into),
        Command::LegacyValidateProject { graph, nodes } => {
            let graph = load_graph_document(&graph)?;
            let registry = NodeRegistry::load_dir(&nodes)?;
            validate_project(&graph, &registry)?;
            println!("valid legacy project: {} {}", graph.id, graph.revision);
            Ok(())
        }
        Command::ValidateProject { project } => {
            let request = load_project_request_v02(project)?;
            if let Err(diagnostics) = validate_project_request_v02(&request) {
                return Err(format_runtime_diagnostics(&diagnostics).into());
            }
            println!(
                "valid project: {} {}",
                request.graph.id, request.graph.revision
            );
            Ok(())
        }
        Command::Plan { project, format } => {
            let request = load_project_request_v02(project)?;
            let (plan, diagnostics) = build_execution_plan_request_v02(&request)
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
        Command::LegacyAudioPlan {
            graph,
            nodes,
            block_size,
            sample_rate,
            format,
        } => {
            let plan = load_audio_plan(
                graph,
                nodes,
                AudioDspPlanOptions {
                    block_size,
                    sample_rate,
                },
            )?;
            match format {
                OutputFormat::Text => {
                    print!("{}", format_audio_dsp_plan_text(&plan));
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
            let request = load_run_project_request_v02(project, frames)?;
            let (plan, diagnostics) = build_execution_plan_run_request_v02(&request)
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
        Command::LegacyPreview {
            graph,
            nodes,
            frames,
        } => {
            let plan = load_plan(graph, nodes)?;
            run_preview_window(plan, PreviewFrameLimit::Frames(frames))
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
            let document = load_preview_document(document)?;
            let frame_limit = if until_close {
                PreviewFrameLimit::UntilClose
            } else {
                PreviewFrameLimit::Frames(frames)
            };
            run_render_preview_window(document, frame_limit, telemetry, control_state)
        }
        Command::LegacyAudioOutput {
            graph,
            nodes,
            block_size,
            duration_ms,
        } => {
            let graph = load_graph_document(&graph)?;
            let registry = NodeRegistry::load_dir(&nodes)?;
            let backend = start_default_audio_output_backend(
                &graph,
                &registry,
                AudioBackendConfig { block_size },
            )?;
            let info = backend.info();
            println!(
                "audio output: device={} sampleRate={} channels={} sampleFormat={}",
                info.device_name, info.sample_rate, info.channels, info.sample_format
            );
            backend.keep_alive_for(Duration::from_millis(duration_ms));
            Ok(())
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

fn load_project_request_v02(
    path: PathBuf,
) -> Result<ProjectRequestV02, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if is_project_document_v02(&value) {
        decode_project_document_request_v02(value)
    } else {
        let request: ProjectRequestV02 = serde_json::from_value(value)?;
        if request.graph.schema_version != "0.2.0" {
            return Err(format!(
                "active project requests require graph.schemaVersion 0.2.0, got {}",
                request.graph.schema_version
            )
            .into());
        }
        Ok(request)
    }
}

fn load_run_project_request_v02(
    path: PathBuf,
    frames: usize,
) -> Result<RunProjectRequestV02, Box<dyn std::error::Error>> {
    let request = load_project_request_v02(path)?;
    Ok(RunProjectRequestV02 {
        document: request.document,
        graph: request.graph,
        nodes: request.nodes,
        patch_library: request.patch_library,
        view_state: request.view_state,
        frames: Some(frames),
    })
}

fn is_project_document_v02(value: &serde_json::Value) -> bool {
    value
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|schema| schema == "skenion.project")
        && value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|version| version == "0.2.0")
}

fn decode_project_document_request_v02(
    mut value: serde_json::Value,
) -> Result<ProjectRequestV02, Box<dyn std::error::Error>> {
    let nodes = value
        .as_object_mut()
        .and_then(|object| object.remove("nodes"))
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    let nodes = serde_json::from_value::<Vec<NodeDefinitionV02>>(nodes)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("frames");
    }
    let document = serde_json::from_value::<ProjectDocumentV02>(value)?;
    if let Err(report) = skenion_contracts::validate_project_document_v02(&document) {
        return Err(report.to_string().into());
    }
    Ok(ProjectRequestV02::from_project_document(document, nodes))
}

fn format_runtime_diagnostics(diagnostics: &[RuntimeDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| match &diagnostic.code {
            Some(code) => format!("{code}: {}", diagnostic.message),
            None => diagnostic.message.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn load_plan(graph: PathBuf, nodes: PathBuf) -> Result<ExecutionPlan, Box<dyn std::error::Error>> {
    let graph = load_graph_document(&graph)?;
    let registry = NodeRegistry::load_dir(&nodes)?;
    Ok(build_execution_plan(&graph, &registry)?)
}

fn load_audio_plan(
    graph: PathBuf,
    nodes: PathBuf,
    options: AudioDspPlanOptions,
) -> Result<AudioDspPlan, Box<dyn std::error::Error>> {
    let graph = load_graph_document(&graph)?;
    let registry = NodeRegistry::load_dir(&nodes)?;
    Ok(build_audio_dsp_plan(&graph, &registry, options)?)
}

fn format_audio_dsp_plan_text(plan: &AudioDspPlan) -> String {
    let mut lines = vec![
        format!("audio dsp plan: {} {}", plan.graph_id, plan.graph_revision),
        format!("blockSize: {}", plan.block_size),
        format!("sampleRate: {}", plan.sample_rate),
        format!("endpoints: {}", plan.endpoints.len()),
        format!("clockDomains: {}", plan.clock_domains.len()),
        format!("partitions: {}", plan.partitions.len()),
        format!("bridgePlans: {}", plan.bridge_plans.len()),
    ];

    for bridge in &plan.bridge_plans {
        lines.push(format!(
            "bridge: {} -> {} method={:?} required={}",
            bridge.source_clock_domain_id,
            bridge.target_clock_domain_id,
            bridge.method,
            bridge.required
        ));
    }

    lines.join("\n") + "\n"
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

fn load_preview_document(path: PathBuf) -> Result<PreviewDocument, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use skenion_runtime::{
        ExecutionModel, ExecutionPlan, GraphDocument, PlanNode, PreviewDocument,
        RENDER_CLEAR_COLOR_KIND, write_preview_document,
    };

    #[test]
    fn binary_target_covers_preview_document_public_surface() {
        let document = PreviewDocument::new(graph(), plan(), 5);
        let path = write_preview_document(&document).expect("document should be written");
        let decoded = super::load_preview_document(path.clone()).expect("document should load");

        assert_eq!(decoded, document);
        std::fs::remove_file(path).expect("test document should be removable");
    }

    fn graph() -> GraphDocument {
        GraphDocument {
            schema: "skenion.graph".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "render-graph".to_owned(),
            revision: "1".to_owned(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    fn plan() -> ExecutionPlan {
        ExecutionPlan {
            graph_id: "render-graph".to_owned(),
            graph_revision: "1".to_owned(),
            nodes: vec![PlanNode {
                node_id: "clear_1".to_owned(),
                kind: RENDER_CLEAR_COLOR_KIND.to_owned(),
                kind_version: "0.1.0".to_owned(),
                execution_model: ExecutionModel::GpuPass,
                order: 0,
            }],
            edges: Vec::new(),
            groups: Vec::new(),
        }
    }
}
