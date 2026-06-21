use std::error::Error;

use tokio::net::TcpListener;

use crate::server::{RuntimeServerState, runtime_router_with_state};

#[derive(Debug, Clone, Copy, Default)]
pub struct ServeRuntimeOptions {
    pub startup_json: bool,
}

pub async fn serve_runtime(host: &str, port: u16) -> Result<(), Box<dyn Error>> {
    serve_runtime_with_options(host, port, ServeRuntimeOptions::default()).await
}

pub async fn serve_runtime_with_options(
    host: &str,
    port: u16,
    options: ServeRuntimeOptions,
) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind((host, port)).await?;
    let local_addr = listener.local_addr()?;
    let state = RuntimeServerState::with_endpoint(local_addr.ip().to_string(), local_addr.port());
    if options.startup_json {
        println!(
            "{}",
            serde_json::to_string(&state.sidecar_startup_response())?
        );
    } else {
        println!("skenion-runtime listening on http://{local_addr}");
    }
    axum::serve(listener, runtime_router_with_state(state)).await?;
    Ok(())
}
