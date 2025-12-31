use config::{Config, File, Environment};
use serde::Deserialize;
use std::env;
use anyhow::Result;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub env: String,
    pub host: String,
    pub http_port: u16,
    
    pub service_version: String,

    // Service URLs
    pub stt_grpc_url: String,
    pub dialog_grpc_url: String,
    pub tts_grpc_url: String,

    // Security (mTLS)
    pub grpc_tls_ca_path: String,
    pub stream_gateway_service_cert_path: String,
    pub stream_gateway_service_key_path: String,

    // [YENİ] Varsayılan Ses Kimliği (Default: coqui:default)
    pub tts_default_voice_id: String,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let builder = Config::builder()
            .add_source(File::with_name(".env").required(false))
            .add_source(Environment::default().separator("__"))
            
            .set_override_option("host", env::var("STREAM_GATEWAY_SERVICE_IPV4_ADDRESS").ok())?
            .set_override_option("http_port", env::var("STREAM_GATEWAY_SERVICE_HTTP_PORT").ok())?
            
            .set_default("env", "production")?
            .set_default("service_version", "0.2.0")?
            .set_default("host", "0.0.0.0")?
            .set_default("http_port", 18030)?
            
            .set_default("stt_grpc_url", "https://stt-gateway-service:15021")?
            .set_default("dialog_grpc_url", "https://dialog-service:12061")?
            .set_default("tts_grpc_url", "https://tts-gateway-service:14011")?

            .set_default("grpc_tls_ca_path", "/sentiric-certificates/certs/ca.crt")?
            .set_default("stream_gateway_service_cert_path", "/sentiric-certificates/certs/stream-gateway-service.crt")?
            .set_default("stream_gateway_service_key_path", "/sentiric-certificates/certs/stream-gateway-service.key")?
            
            // [YENİ] Varsayılan olarak Coqui kullan (Kalite öncelikli)
            .set_default("tts_default_voice_id", "coqui:default")?;

        builder.build()?.try_deserialize().map_err(|e| e.into())
    }
}