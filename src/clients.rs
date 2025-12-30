use crate::config::AppConfig;
use anyhow::{Context, Result};
use sentiric_contracts::sentiric::{
    stt::v1::stt_gateway_service_client::SttGatewayServiceClient,
    dialog::v1::dialog_service_client::DialogServiceClient,
    tts::v1::tts_gateway_service_client::TtsGatewayServiceClient,
};
use tonic::transport::{Channel, ClientTlsConfig, Certificate, Identity, Endpoint};
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct GrpcClients {
    pub stt: SttGatewayServiceClient<Channel>,
    pub dialog: DialogServiceClient<Channel>,
    pub tts: TtsGatewayServiceClient<Channel>,
}

impl GrpcClients {
    pub async fn connect(config: &Arc<AppConfig>) -> Result<Self> {
        info!("🔌 Configuring backend services (Lazy Connection Mode)...");

        let tls_config = load_tls_config(config).await?;

        // 1. STT Service (Lazy)
        let stt_channel = Endpoint::from_shared(config.stt_grpc_url.clone())?
            .tls_config(tls_config.clone())?
            .connect_lazy(); // Değişiklik burada: await yok, hata fırlatmaz.

        // 2. Dialog Service (Lazy)
        let dialog_channel = Endpoint::from_shared(config.dialog_grpc_url.clone())?
            .tls_config(tls_config.clone())?
            .connect_lazy();

        // 3. TTS Service (Lazy)
        let tts_channel = Endpoint::from_shared(config.tts_grpc_url.clone())?
            .tls_config(tls_config.clone())?
            .connect_lazy();

        info!("✅ Backend service endpoints configured. Connections will be established on first request.");

        Ok(Self {
            stt: SttGatewayServiceClient::new(stt_channel),
            dialog: DialogServiceClient::new(dialog_channel),
            tts: TtsGatewayServiceClient::new(tts_channel),
        })
    }
}

async fn load_tls_config(config: &AppConfig) -> Result<ClientTlsConfig> {
    let ca_pem = tokio::fs::read(&config.grpc_tls_ca_path).await
        .context(format!("CA cert not found at: {}", config.grpc_tls_ca_path))?;
    let ca = Certificate::from_pem(ca_pem);

    let cert = tokio::fs::read(&config.stream_gateway_service_cert_path).await
        .context(format!("Client cert not found at: {}", config.stream_gateway_service_cert_path))?;
    
    let key = tokio::fs::read(&config.stream_gateway_service_key_path).await
        .context(format!("Client key not found at: {}", config.stream_gateway_service_key_path))?;
    
    let identity = Identity::from_pem(cert, key);

    Ok(ClientTlsConfig::new()
        .domain_name("sentiric.cloud")
        .ca_certificate(ca)
        .identity(identity))
}