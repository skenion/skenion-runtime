use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use skenion_runtime::{
    AudioBackendConfig, AudioDspPlan, AudioDspPlanOptions, DEFAULT_HOST, DEFAULT_PORT,
    ExecutionPlan, NodeRegistry, PreviewDocument, PreviewFrameLimit, build_audio_dsp_plan,
    build_execution_plan, format_dummy_execution_text, format_plan_text, load_graph_document,
    load_node_definition, run_dummy_execution, run_preview_window, run_render_preview_window,
    serve_runtime, start_default_audio_output_backend, validate_project,
};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate a Skenion Node Definition Manifest v0.1 JSON file.
    ValidateNode {
        /// Path to the node definition manifest.
        path: PathBuf,
    },
    /// Validate a Skenion Graph Document v0.1 JSON file.
    ValidateGraph {
        /// Path to the graph document.
        path: PathBuf,
    },
    /// Validate a graph against a node definition registry.
    ValidateProject {
        /// Path to the graph document.
        #[arg(long)]
        graph: PathBuf,
        /// Directory containing node definition manifests.
        #[arg(long)]
        nodes: PathBuf,
    },
    /// Build an execution plan skeleton for a graph and node registry.
    Plan {
        /// Path to the graph document.
        #[arg(long)]
        graph: PathBuf,
        /// Directory containing node definition manifests.
        #[arg(long)]
        nodes: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Build an audio DSP plan with endpoint, clock-domain, and bridge metadata.
    AudioPlan {
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
        /// Path to the graph document.
        #[arg(long)]
        graph: PathBuf,
        /// Directory containing node definition manifests.
        #[arg(long)]
        nodes: PathBuf,
        /// Number of dummy frames to simulate.
        #[arg(long, default_value_t = 1)]
        frames: usize,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Open a local placeholder preview window driven by the execution plan.
    Preview {
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
    /// Run the CPAL default output backend for an audio.output DSP graph.
    AudioOutput {
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
    /// Start the local HTTP JSON control API.
    Serve {
        /// Host to bind. Defaults to localhost for local development safety.
        #[arg(long, default_value = DEFAULT_HOST)]
        host: String,
        /// Port to bind.
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
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
        Command::ValidateNode { path } => load_node_definition(&path)
            .map(|definition| {
                println!(
                    "valid node definition: {} {}",
                    definition.id, definition.version
                );
            })
            .map_err(Into::into),
        Command::ValidateGraph { path } => load_graph_document(&path)
            .map(|graph| {
                println!("valid graph: {} {}", graph.id, graph.revision);
            })
            .map_err(Into::into),
        Command::ValidateProject { graph, nodes } => {
            let graph = load_graph_document(&graph)?;
            let registry = NodeRegistry::load_dir(&nodes)?;
            validate_project(&graph, &registry)?;
            println!("valid project: {} {}", graph.id, graph.revision);
            Ok(())
        }
        Command::Plan {
            graph,
            nodes,
            format,
        } => {
            let plan = load_plan(graph, nodes)?;
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
        Command::AudioPlan {
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
            graph,
            nodes,
            frames,
            format,
        } => {
            let plan = load_plan(graph, nodes)?;
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
        Command::Preview {
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
        Command::AudioOutput {
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
        Command::Serve { host, port } => serve_runtime(&host, port).await,
    }
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
