use std::error::Error;

use tokio::net::TcpListener;

use crate::server::runtime_router;

pub async fn serve_runtime(host: &str, port: u16) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind((host, port)).await?;
    let local_addr = listener.local_addr()?;
    println!("skenion-runtime listening on http://{local_addr}");
    axum::serve(listener, runtime_router()).await?;
    Ok(())
}
