mod config;
mod clients;
mod handlers;

use anyhow::Result;
use axum::Router;
use std::sync::Arc;
use std::net::SocketAddr;
use tower_http::services::ServeDir;
use tracing::info;
use metrics_exporter_prometheus::PrometheusBuilder;
use crate::config::AppConfig;
use crate::clients::GrpcClients;
use crate::handlers::ws_handler;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Loglama Başlat
    tracing_subscriber::fmt::init();
    
    // 2. Config Yükle
    let config = Arc::new(AppConfig::load()?);
    info!("🚀 Stream Gateway Service v{} starting...", config.service_version);

    // 3. Metrics Exporter Başlat (Prometheus)
    // DÜZELTME: install_expectations() kaldırıldı. Doğrudan recorder kuruluyor.
    let builder = PrometheusBuilder::new();
    let handle = builder.install_recorder()?;

    // Metrics Endpoint için ayrı bir router/listener (Port: 18032)
    // config.host genellikle 0.0.0.0'dır.
    let metrics_addr: SocketAddr = format!("{}:18032", config.host).parse()?;
    let metrics_app = Router::new().route("/metrics", axum::routing::get(move || std::future::ready(handle.render())));
    
    tokio::spawn(async move {
        info!("📊 Metrics listening on {}", metrics_addr);
        let listener = tokio::net::TcpListener::bind(metrics_addr).await.unwrap();
        axum::serve(listener, metrics_app).await.unwrap();
    });

    // 4. Connect to Microservices
    let clients = Arc::new(GrpcClients::connect(&config).await?);

    // 5. Setup Main Router
    let app = Router::new()
        .route("/ws", axum::routing::get(ws_handler))
        .nest_service("/", ServeDir::new("static")) // Test UI
        .with_state(clients);

    // 6. Start Main Server (Port: 18030)
    let addr: SocketAddr = format!("{}:{}", config.host, config.http_port).parse()?;
    info!("🌐 Gateway HTTP/WS listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}