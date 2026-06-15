use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use skenion_runtime::{
    ExecutionPlan, NodeRegistry, build_execution_plan, format_dummy_execution_text,
    format_plan_text, load_graph_document, load_node_definition, run_dummy_execution,
    run_preview_window, validate_project,
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
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

fn main() {
    let cli = Cli::parse();

    if let Err(error) = run(cli) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
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
            run_preview_window(plan, frames)
        }
    }
}

fn load_plan(graph: PathBuf, nodes: PathBuf) -> Result<ExecutionPlan, Box<dyn std::error::Error>> {
    let graph = load_graph_document(&graph)?;
    let registry = NodeRegistry::load_dir(&nodes)?;
    Ok(build_execution_plan(&graph, &registry)?)
}
