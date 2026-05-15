#![allow(dead_code)]
use crate::config::AppConfig;
use crate::pubsub::ghost_publisher::GhostPublisher;
use sentiric_contracts::sentiric::event::v1::{
    CognitiveMapUpdatedEvent, MediaGenerationCompletedEvent,
};
use sentiric_contracts::sentiric::video::v1::video_gateway_service_client::VideoGatewayServiceClient;
use tokio::sync::broadcast;
use tonic::transport::Channel;

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub ghost_publisher: GhostPublisher,
    // Crystalline Zihin Haritaları için
    pub cognitive_tx: broadcast::Sender<CognitiveMapUpdatedEvent>,
    // [YENİ] Medya Üretim Sonuçları için
    pub media_tx: broadcast::Sender<MediaGenerationCompletedEvent>,
    // [YENİ] Video Gateway İstemcisi
    pub video_client: Option<VideoGatewayServiceClient<Channel>>,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        video_client: Option<VideoGatewayServiceClient<Channel>>,
    ) -> Self {
        let publisher = GhostPublisher::new(config.rabbitmq_url.clone(), config.tenant_id.clone());
        let (cognitive_tx, _) = broadcast::channel(1024);
        let (media_tx, _) = broadcast::channel(1024);

        Self {
            config,
            ghost_publisher: publisher,
            cognitive_tx,
            media_tx,
            video_client,
        }
    }
}
