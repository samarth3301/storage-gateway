use std::time::Instant;
use tokio::signal;
use tracing::{error, info};

use s3_cdn_network::app::{create_app, AppState};
use s3_cdn_network::config::Config;
use s3_cdn_network::telemetry;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    dotenvy::dotenv().ok();
    telemetry::init_logger()?;
    telemetry::metrics_handle();
    tokio::spawn(telemetry::run_metrics_upkeep());

    let start_time = Instant::now();
    let config = Config::from_env().inspect_err(|e| error!("Configuration error: {}", e))?;
    let state = AppState::init(&config).await?;
    let app = create_app(state);

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .inspect_err(|e| error!("Failed to bind to {}: {}", addr, e))?;
    info!("Listening on {} (startup {:?})", addr, start_time.elapsed());

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .inspect_err(|e| error!("Server error: {}", e))?;

    info!("Server shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, starting graceful shutdown...");
}
