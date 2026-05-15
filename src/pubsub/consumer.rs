use lapin::{options::*, types::FieldTable, Connection, ConnectionProperties};
use prost::Message;
use sentiric_contracts::sentiric::event::v1::{
    CognitiveMapUpdatedEvent, MediaGenerationCompletedEvent,
};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::sleep;
use tracing::{error, info};

pub struct CognitiveConsumer;

impl CognitiveConsumer {
    pub async fn start(rmq_url: String, tx: broadcast::Sender<CognitiveMapUpdatedEvent>) {
        tokio::spawn(async move {
            let clean_url = rmq_url.trim().replace("\"", "");
            if clean_url.is_empty() {
                return;
            }

            loop {
                if let Ok(conn) =
                    Connection::connect(&clean_url, ConnectionProperties::default()).await
                {
                    if let Ok(channel) = conn.create_channel().await {
                        let q = channel
                            .queue_declare(
                                "",
                                QueueDeclareOptions {
                                    exclusive: true,
                                    auto_delete: true,
                                    ..Default::default()
                                },
                                FieldTable::default(),
                            )
                            .await
                            .unwrap();
                        let _ = channel
                            .queue_bind(
                                q.name().as_str(),
                                "sentiric_events",
                                "cognitive.map.updated",
                                QueueBindOptions::default(),
                                FieldTable::default(),
                            )
                            .await;

                        if let Ok(mut cons) = channel
                            .basic_consume(
                                q.name().as_str(),
                                "stream_gw_cog_worker",
                                BasicConsumeOptions::default(),
                                FieldTable::default(),
                            )
                            .await
                        {
                            while let Some(Ok(delivery)) = futures::StreamExt::next(&mut cons).await
                            {
                                match CognitiveMapUpdatedEvent::decode(&*delivery.data) {
                                    Ok(event) => {
                                        let _ = tx.send(event);
                                    }
                                    Err(e) => {
                                        error!(event="PROTO_DECODE_ERR", error=%e, "Failed to decode CognitiveMapUpdatedEvent");
                                    }
                                }
                                let _ = delivery.ack(BasicAckOptions::default()).await;
                            }
                        }
                    }
                }
                sleep(Duration::from_secs(5)).await;
            }
        });
    }
}

pub struct MediaConsumer;

impl MediaConsumer {
    pub async fn start(rmq_url: String, tx: broadcast::Sender<MediaGenerationCompletedEvent>) {
        tokio::spawn(async move {
            let clean_url = rmq_url.trim().replace("\"", "");
            if clean_url.is_empty() {
                return;
            }

            loop {
                if let Ok(conn) =
                    Connection::connect(&clean_url, ConnectionProperties::default()).await
                {
                    if let Ok(channel) = conn.create_channel().await {
                        let q = channel
                            .queue_declare(
                                "",
                                QueueDeclareOptions {
                                    exclusive: true,
                                    auto_delete: true,
                                    ..Default::default()
                                },
                                FieldTable::default(),
                            )
                            .await
                            .unwrap();

                        let _ = channel
                            .queue_bind(
                                q.name().as_str(),
                                "sentiric_events",
                                "media.generation.completed",
                                QueueBindOptions::default(),
                                FieldTable::default(),
                            )
                            .await;
                        let _ = channel
                            .queue_bind(
                                q.name().as_str(),
                                "sentiric_events",
                                "media.generation.failed",
                                QueueBindOptions::default(),
                                FieldTable::default(),
                            )
                            .await;

                        if let Ok(mut cons) = channel
                            .basic_consume(
                                q.name().as_str(),
                                "stream_gw_media_worker",
                                BasicConsumeOptions::default(),
                                FieldTable::default(),
                            )
                            .await
                        {
                            info!(
                                event = "MEDIA_CONSUMER_READY",
                                "🎬 Listening for media generation events."
                            );
                            while let Some(Ok(delivery)) = futures::StreamExt::next(&mut cons).await
                            {
                                match MediaGenerationCompletedEvent::decode(&*delivery.data) {
                                    Ok(event) => {
                                        let _ = tx.send(event);
                                    }
                                    Err(e) => {
                                        error!(event="PROTO_DECODE_ERR", error=%e, "Failed to decode MediaGenerationCompletedEvent");
                                    }
                                }
                                let _ = delivery.ack(BasicAckOptions::default()).await;
                            }
                        }
                    }
                }
                sleep(Duration::from_secs(5)).await;
            }
        });
    }
}
