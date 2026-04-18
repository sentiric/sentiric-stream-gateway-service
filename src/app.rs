#![allow(dead_code)]
use crate::config::AppConfig;
use crate::pubsub::ghost_publisher::GhostPublisher;
use sentiric_contracts::sentiric::event::v1::CognitiveMapUpdatedEvent;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub ghost_publisher: GhostPublisher,
    // [ARCH-COMPLIANCE FIX]: UI'a basılacak canlı Zihin Haritaları için Broadcaster
    pub cognitive_tx: broadcast::Sender<CognitiveMapUpdatedEvent>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        let publisher = GhostPublisher::new(config.rabbitmq_url.clone(), config.tenant_id.clone());
        let (cognitive_tx, _) = broadcast::channel(1024);
        Self {
            config,
            ghost_publisher: publisher,
            cognitive_tx,
        }
    }
}
