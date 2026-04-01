mod app;
mod config;
mod pubsub;
mod server;

use crate::app::AppState;
use axum::{routing::get, Router};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // DEĞİŞİKLİK 1
    let subscriber = FmtSubscriber::builder()
        .json()
        .with_max_level(Level::INFO)
        .with_current_span(false)
        .with_span_list(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("Setting default subscriber failed");

    let worker_threads: usize = std::env::var("WORKER_THREADS")
        .unwrap_or_else(|_| "2".to_string())
        .parse()
        .unwrap_or(2);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()
        .expect("Failed building the Runtime");

    runtime.block_on(async {
        let config = match config::AppConfig::load() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(event="CONFIG_LOAD_FAIL", error=%e, "Config yüklenemedi!");
                return;
            }
        };
        let port = config.port;
        let tenant_id = config.tenant_id.clone();
        let app_state = Arc::new(AppState::new(config));

        let app = Router::new().route("/healthz", get(server::http::healthz)).route("/ws", get(server::ws_handler::ws_upgrade)).with_state(app_state);

        let listener = match TcpListener::bind(format!("0.0.0.0:{}", port)).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(event="PORT_BIND_FAIL", port=port, error=%e, "Port dinlemeye açılamadı!");
                return;
            }
        };

        info!(event = "SERVER_READY", tenant_id = %tenant_id, port = port, "Stream Gateway listening.");

        if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal(tenant_id)).await {
            tracing::error!(event="SERVER_CRASH", error=%e, "Axum HTTP sunucusu çöktü");
        }
    });

    Ok(())
}

async fn shutdown_signal(tenant_id: String) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!(
        event = "SERVICE_STOPPED",
        tenant_id = %tenant_id,
        "Graceful shutdown complete."
    );
}
