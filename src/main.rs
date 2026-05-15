mod app;
mod config;
mod pubsub;
mod server;
mod telemetry;

use crate::app::AppState;
use crate::telemetry::SutsFormatter;
use axum::{routing::get, Router};
use sentiric_contracts::sentiric::video::v1::video_gateway_service_client::VideoGatewayServiceClient;
use std::io::Write;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tonic::transport::{Certificate, ClientTlsConfig, Endpoint, Identity};
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter, Registry};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = match config::AppConfig::load() {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "{{\"schema_v\":\"1.0.0\",\"severity\":\"FATAL\",\"event\":\"CONFIG_ERROR\",\"message\":\"{}\"}}", e);
            std::process::exit(1);
        }
    };

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let suts_formatter = SutsFormatter::new(
        "stream-gateway-service".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
        config.env.clone(),
        config.tenant_id.clone(),
    );

    let subscriber = Registry::default()
        .with(env_filter)
        .with(fmt::layer().event_format(suts_formatter));
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

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
        let port = config.port;
        let tenant_id = config.tenant_id.clone();
        let rmq_url = config.rabbitmq_url.clone();

        // [ARCH-COMPLIANCE FIX]: Video Gateway İstemcisini mTLS ile kuruyoruz
        let video_client = async {
            if let (Ok(ca_cert), Ok(cert), Ok(key)) = (
                tokio::fs::read(&config.tls_ca_path).await,
                tokio::fs::read(&config.tls_cert_path).await,
                tokio::fs::read(&config.tls_key_path).await
            ) {
                let ca = Certificate::from_pem(ca_cert);
                let identity = Identity::from_pem(cert, key);
                let tls_config = ClientTlsConfig::new().domain_name("sentiric.cloud").ca_certificate(ca).identity(identity);

                let url = std::env::var("VIDEO_GATEWAY_GRPC_URL").unwrap_or_else(|_| "https://video-gateway-service:16101".to_string());

                // FIX: connect_lazy() Channel döner, Result dönmez.
                let channel = Endpoint::from_shared(url).unwrap().tls_config(tls_config).unwrap().connect_lazy();
                tracing::info!(event="VIDEO_CLIENT_READY", "Video Gateway client configured.");
                return Some(VideoGatewayServiceClient::new(channel));
            }
            tracing::warn!(event="VIDEO_CLIENT_DISABLED", "Could not configure Video Gateway client.");
            None
        }.await;

        let app_state = Arc::new(AppState::new(config.clone(), video_client));

        crate::pubsub::consumer::CognitiveConsumer::start(rmq_url.clone(), app_state.cognitive_tx.clone()).await;
        crate::pubsub::consumer::MediaConsumer::start(rmq_url.clone(), app_state.media_tx.clone()).await;

        let app = Router::new()
            .route("/healthz", get(server::http::healthz))
            .route("/ws", get(server::ws_handler::ws_upgrade))
            .with_state(app_state);

        let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await.unwrap();
        info!(event = "SERVER_READY", tenant_id = %tenant_id, port = port, "Stream Gateway listening.");

        axum::serve(listener, app).with_graceful_shutdown(shutdown_signal(tenant_id)).await.unwrap();
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
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {}, }
    info!(event = "SERVICE_STOPPED", tenant_id = %tenant_id, "Graceful shutdown complete.");
}
