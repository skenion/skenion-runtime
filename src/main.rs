use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use skenion_runtime::{
    DEFAULT_HOST, DEFAULT_PORT, ExecutionPlan, PreviewFrameLimit, ServeRuntimeOptions,
    format_midi_clock_fixture_report_text, run_midi_clock_fixture_file, run_preview_window,
    run_render_preview_document_file, serve_runtime, serve_runtime_with_options,
};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
        /// Print machine-readable local sidecar startup JSON after binding.
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
}
