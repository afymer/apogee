use tracing::info;

fn main() {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).expect("Couldn't register the logger");

    info!("Hey!");

    apogee_app::run();
}
