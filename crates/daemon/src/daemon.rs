use std::net::SocketAddr;

use gphoto::actor::CameraActor;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("Starting Apogee daemon...");

    let camera_handle = CameraActor::spawn();

    let bind_addr: SocketAddr = std::env::var("APOGEE_DAEMON_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:4000".to_string())
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 4000)));

    server::start_server(bind_addr, camera_handle).await?;

    Ok(())
}
