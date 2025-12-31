use crate::config::AppConfig;
use anyhow::{Context, Result};
use sentiric_contracts::sentiric::{
    stt::v1::stt_gateway_service_client::SttGatewayServiceClient,
    dialog::v1::dialog_service_client::DialogServiceClient,
    tts::v1::tts_gateway_service_client::TtsGatewayServiceClient,
};
use tonic::transport::{Channel, ClientTlsConfig, Certificate, Identity, Endpoint};
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Clone)]
pub struct GrpcClients {
    pub stt: SttGatewayServiceClient<Channel>,
    pub dialog: DialogServiceClient<Channel>,
    pub tts: TtsGatewayServiceClient<Channel>,
    pub default_voice_id: String,
}

impl GrpcClients {
    pub async fn connect(config: &Arc<AppConfig>) -> Result<Self> {
        info!("🔌 Configuring backend services...");

        // 1. Herhangi bir servis HTTPS kullanıyor mu kontrol et
        let needs_tls = config.stt_grpc_url.starts_with("https") || 
                        config.dialog_grpc_url.starts_with("https") || 
                        config.tts_grpc_url.starts_with("https");

        // 2. Gerekiyorsa TLS konfigürasyonunu yükle (Sadece bir kere)
        let tls_config = if needs_tls {
            if !config.grpc_tls_ca_path.is_empty() {
                info!("🔐 HTTPS detected, loading TLS certificates...");
                Some(load_tls_config(config).await?)
            } else {
                warn!("⚠️ HTTPS detected but TLS path is empty! Will attempt connection without TLS config (risky).");
                None
            }
        } else {
            None
        };

        // 3. Kanalları oluştur
        let stt_channel = connect_endpoint(&config.stt_grpc_url, "STT", &tls_config).await?;
        let dialog_channel = connect_endpoint(&config.dialog_grpc_url, "Dialog", &tls_config).await?;
        let tts_channel = connect_endpoint(&config.tts_grpc_url, "TTS", &tls_config).await?;

        info!("✅ Backend service endpoints configured. Voice ID: {}", config.tts_default_voice_id);

        Ok(Self {
            stt: SttGatewayServiceClient::new(stt_channel),
            dialog: DialogServiceClient::new(dialog_channel),
            tts: TtsGatewayServiceClient::new(tts_channel),
            default_voice_id: config.tts_default_voice_id.clone(),
        })
    }
}

// Yardımcı Fonksiyon (Async Closure yerine)
async fn connect_endpoint(url: &str, name: &str, tls_config: &Option<ClientTlsConfig>) -> Result<Channel> {
    let endpoint = Endpoint::from_shared(url.to_string())
        .context(format!("Invalid URL for {}", name))?;

    if url.starts_with("https") {
        if let Some(tls) = tls_config {
            info!("🔒 [{}] Connecting Securely to: {}", name, url);
            Ok(endpoint.tls_config(tls.clone())?.connect_lazy())
        } else {
            warn!("⚠️ [{}] URL is HTTPS but TLS config missing. Connecting INSECURE to: {}", name, url);
            Ok(endpoint.connect_lazy())
        }
    } else {
        info!("🔓 [{}] Connecting Insecurely to: {}", name, url);
        Ok(endpoint.connect_lazy())
    }
}

async fn load_tls_config(config: &AppConfig) -> Result<ClientTlsConfig> {
    let ca_pem = tokio::fs::read(&config.grpc_tls_ca_path).await
        .context(format!("CA cert not found at: '{}'", config.grpc_tls_ca_path))?;
    let ca = Certificate::from_pem(ca_pem);

    let cert = tokio::fs::read(&config.stream_gateway_service_cert_path).await
        .context(format!("Client cert not found at: '{}'", config.stream_gateway_service_cert_path))?;
    
    let key = tokio::fs::read(&config.stream_gateway_service_key_path).await
        .context(format!("Client key not found at: '{}'", config.stream_gateway_service_key_path))?;
    
    let identity = Identity::from_pem(cert, key);

    Ok(ClientTlsConfig::new()
        .domain_name("sentiric.cloud")
        .ca_certificate(ca)
        .identity(identity))
}