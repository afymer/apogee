use std::net::SocketAddr;

use controller::Controller;
use gphoto::actor::CameraActor;
use synscan::MountActor;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

mod server;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .init();

    let sim_mode = std::env::args().any(|a| a == "--sim" || a == "--simulate")
        || matches!(
            std::env::var("APOGEE_SIM").ok().as_deref(),
            Some("1" | "true" | "TRUE" | "yes")
        );

    let (camera_handle, mount_handle) = if sim_mode {
        info!("Starting Apogee daemon in SIMULATED hardware mode (camera + NEQ6-Pro mount)...");
        (
            CameraActor::spawn_simulated(),
            MountActor::spawn_simulated(),
        )
    } else {
        info!("Starting Apogee daemon...");
        (CameraActor::spawn(), MountActor::spawn())
    };
    let controller_handle = Controller::spawn_with_mount(camera_handle, mount_handle);

    let bind_addr: SocketAddr = match std::env::var("APOGEE_DAEMON_ADDR") {
        Ok(raw_addr) => match raw_addr.parse() {
            Ok(addr) => addr,
            Err(err) => {
                warn!(
                    "Invalid APOGEE_DAEMON_ADDR '{raw_addr}': {err}. Falling back to 0.0.0.0:4000"
                );
                SocketAddr::from(([0, 0, 0, 0], 4000))
            }
        },
        Err(_) => SocketAddr::from(([0, 0, 0, 0], 4000)),
    };

    server::start_server(bind_addr, controller_handle).await?;

    Ok(())
}
