#![allow(dead_code)]
use crate::config::AppConfig;
use crate::pubsub::ghost_publisher::GhostPublisher;

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub ghost_publisher: GhostPublisher,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        // Hata Düzeltme: Unused Arc import silindi
        let publisher = GhostPublisher::new(config.rabbitmq_url.clone(), config.tenant_id.clone());
        Self {
            config,
            ghost_publisher: publisher,
        }
    }
}
