// File: src/pubsub/consumer.rs
use lapin::{options::*, types::FieldTable, Connection, ConnectionProperties};
use prost::Message; // Decode yeteneği için gereklidir.
use sentiric_contracts::sentiric::event::v1::CognitiveMapUpdatedEvent;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub struct CognitiveConsumer;

impl CognitiveConsumer {
    pub async fn start(rmq_url: String, tx: broadcast::Sender<CognitiveMapUpdatedEvent>) {
        tokio::spawn(async move {
            let clean_url = rmq_url.trim().replace("\"", "");
            if clean_url.is_empty() {
                warn!(
                    event = "MQ_DISABLED",
                    "Ghost Mode Active: No RabbitMQ URL. Cognitive Sync to UI disabled."
                );
                return;
            }

            loop {
                match Connection::connect(&clean_url, ConnectionProperties::default()).await {
                    Ok(conn) => {
                        if let Ok(channel) = conn.create_channel().await {
                            let queue = channel
                                .queue_declare(
                                    "",
                                    QueueDeclareOptions {
                                        exclusive: true,
                                        auto_delete: true,
                                        ..Default::default()
                                    },
                                    FieldTable::default(),
                                )
                                .await;

                            if let Ok(q) = queue {
                                if channel
                                    .queue_bind(
                                        q.name().as_str(),
                                        "sentiric_events",
                                        "cognitive.map.updated",
                                        QueueBindOptions::default(),
                                        FieldTable::default(),
                                    )
                                    .await
                                    .is_ok()
                                {
                                    let consumer = channel
                                        .basic_consume(
                                            q.name().as_str(),
                                            "stream_gw_cog_worker",
                                            BasicConsumeOptions::default(),
                                            FieldTable::default(),
                                        )
                                        .await;

                                    if let Ok(mut cons) = consumer {
                                        info!(event="COGNITIVE_CONSUMER_READY", "Listening for cognitive maps to broadcast to WebSockets.");
                                        while let Some(delivery) =
                                            futures::StreamExt::next(&mut cons).await
                                        {
                                            if let Ok(delivery) = delivery {
                                                if let Ok(event) = CognitiveMapUpdatedEvent::decode(
                                                    &*delivery.data,
                                                ) {
                                                    let _ = tx.send(event);
                                                } else {
                                                    error!(
                                                        event = "PROTO_UNMARSHAL_FAIL",
                                                        "Failed to decode CognitiveMapUpdatedEvent"
                                                    );
                                                }
                                                let _ =
                                                    delivery.ack(BasicAckOptions::default()).await;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(event="MQ_CONNECT_FAIL", error=%e, "RabbitMQ unreachable for Cognitive Consumer. Retrying...");
                    }
                }
                sleep(Duration::from_secs(5)).await;
            }
        });
    }
}
