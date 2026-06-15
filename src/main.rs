use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use skenion_runtime::{
    NodeRegistry, build_execution_plan, format_plan_text, load_graph_document,
    load_node_definition, validate_project,
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
        #[arg(long, value_enum, default_value_t = PlanFormat::Text)]
        format: PlanFormat,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PlanFormat {
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
            let graph = load_graph_document(&graph)?;
            let registry = NodeRegistry::load_dir(&nodes)?;
            let plan = build_execution_plan(&graph, &registry)?;
            match format {
                PlanFormat::Text => {
                    print!("{}", format_plan_text(&plan));
                }
                PlanFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&plan)?);
                }
            }
            Ok(())
        }
    }
}
