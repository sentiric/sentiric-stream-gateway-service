mod config;
mod clients;
mod handlers;

use anyhow::Result;
use axum::Router;
use std::sync::Arc;
use std::net::SocketAddr;
use tower_http::services::ServeDir;
use tracing::info;
use crate::config::AppConfig;
use crate::clients::GrpcClients;
use crate::handlers::ws_handler;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    
    // 1. Load Config
    let config = Arc::new(AppConfig::load()?);
    // GÜNCELLEME: İsim ve Versiyon
    info!("🚀 Stream Gateway Service v{} starting...", config.service_version);

    // 2. Connect to Microservices
    let clients = Arc::new(GrpcClients::connect(&config).await?);

    // 3. Setup Router
    let app = Router::new()
        .route("/ws", axum::routing::get(ws_handler))
        .nest_service("/", ServeDir::new("static")) // Test UI
        .with_state(clients);

    // 4. Start Server
    let addr: SocketAddr = format!("{}:{}", config.host, config.http_port).parse()?;
    info!("🌐 Gateway HTTP/WS listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}