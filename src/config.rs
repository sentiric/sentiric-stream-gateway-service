#[derive(Debug, Clone)]
pub struct AppConfig {
    pub port: u16,
    pub tenant_id: String,

    // Target gRPC URLs
    pub stt_gateway_url: String,
    pub dialog_service_url: String,
    pub tts_gateway_url: String,

    // mTLS Paths
    pub tls_ca_path: String,
    pub tls_cert_path: String,
    pub tls_key_path: String,

    // RabbitMQ
    pub rabbitmq_url: String,
}

impl AppConfig {
    pub fn load() -> Result<Self, String> {
        // DEĞİŞİKLİK 1
        let tenant_id = std::env::var("TENANT_ID").unwrap_or_default();
        if tenant_id.trim().is_empty() {
            return Err("[ARCH-COMPLIANCE] TENANT_ID is MANDATORY.".into());
        }

        let stt_url = std::env::var("STT_GATEWAY_GRPC_URL").unwrap_or_default();
        let dialog_url = std::env::var("DIALOG_SERVICE_GRPC_URL").unwrap_or_default();
        let tts_url = std::env::var("TTS_GATEWAY_GRPC_URL").unwrap_or_default();

        if stt_url.starts_with("http://")
            || dialog_url.starts_with("http://")
            || tts_url.starts_with("http://")
        {
            return Err(
                "[ARCH-COMPLIANCE] Insecure HTTP target URLs are strictly forbidden.".into(),
            );
        }

        let tls_ca_path =
            std::env::var("GRPC_TLS_CA_PATH").map_err(|_| "GRPC_TLS_CA_PATH missing.")?;
        let tls_cert_path = std::env::var("STREAM_GATEWAY_SERVICE_CERT_PATH")
            .map_err(|_| "STREAM_GATEWAY_SERVICE_CERT_PATH missing.")?;
        let tls_key_path = std::env::var("STREAM_GATEWAY_SERVICE_KEY_PATH")
            .map_err(|_| "STREAM_GATEWAY_SERVICE_KEY_PATH missing.")?;

        Ok(Self {
            port: std::env::var("STREAM_GATEWAY_SERVICE_HTTP_PORT")
                .unwrap_or_else(|_| "18030".to_string())
                .parse()
                .unwrap_or(18030),
            tenant_id,
            stt_gateway_url: stt_url,
            dialog_service_url: dialog_url,
            tts_gateway_url: tts_url,
            tls_ca_path,
            tls_cert_path,
            tls_key_path,
            rabbitmq_url: std::env::var("RABBITMQ_URL").unwrap_or_default(),
        })
    }
}
