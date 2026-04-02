// File: sentiric-stream-gateway-service/src/pubsub/ghost_publisher.rs
#![allow(dead_code)]
use lapin::{options::*, BasicProperties, Connection, ConnectionProperties};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tracing::{info, warn}; // [FIX]: error macro'su kaldırıldı, yerine warn kullanılacak

const MAX_BUFFER_CAPACITY: usize = 1000;
const MAX_BACKOFF_SECS: u64 = 60;

#[derive(Clone)]
pub struct GhostPublisher {
    buffer: Arc<Mutex<VecDeque<(String, Value)>>>,
}

impl GhostPublisher {
    pub fn new(rabbitmq_url: String, tenant_id: String) -> Self {
        let buffer: Arc<Mutex<VecDeque<(String, Value)>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(MAX_BUFFER_CAPACITY)));
        let buffer_clone = buffer.clone();

        tokio::spawn(async move {
            let clean_url = rabbitmq_url.trim().replace("\"", "");

            if clean_url.is_empty() {
                warn!(event="MQ_DISABLED", tenant_id=%tenant_id, "Ghost Mode Active: RabbitMQ URL not provided. Events will be drained in memory.");
                loop {
                    sleep(Duration::from_secs(10)).await;
                    let mut b = buffer_clone.lock().await;
                    b.clear();
                }
            } else {
                let mut backoff = 1;

                loop {
                    match Connection::connect(&clean_url, ConnectionProperties::default()).await {
                        Ok(conn) => {
                            backoff = 1;
                            info!(event="MQ_CONNECTED", tenant_id=%tenant_id, "Connected to RabbitMQ. Draining Ghost Buffer...");

                            if let Ok(channel) = conn.create_channel().await {
                                loop {
                                    let msg = {
                                        let mut b = buffer_clone.lock().await;
                                        b.pop_front()
                                    };

                                    if let Some((routing_key, payload)) = msg {
                                        let payload_bytes =
                                            serde_json::to_vec(&payload).unwrap_or_default();
                                        let res = channel
                                            .basic_publish(
                                                "sentiric_events",
                                                &routing_key,
                                                BasicPublishOptions::default(),
                                                payload_bytes.as_slice(),
                                                BasicProperties::default(),
                                            )
                                            .await;

                                        if res.is_err() {
                                            let mut b = buffer_clone.lock().await;
                                            if b.len() >= MAX_BUFFER_CAPACITY {
                                                warn!(event="GHOST_BUFFER_FULL_DISCARD", tenant_id=%tenant_id, "Ring buffer full. Dropping oldest message.");
                                            } else {
                                                b.push_front((routing_key, payload));
                                            }
                                            break;
                                        }
                                    } else {
                                        sleep(Duration::from_millis(100)).await;
                                    }

                                    if conn.status().state() != lapin::ConnectionState::Connected {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            // [CRITICAL FIX]: error!() yerine warn!() kullanıldı. Çünkü bu beklenen bir Degraded (Ghost) durumudur.
                            warn!(event="MQ_CONNECT_FAIL", tenant_id=%tenant_id, backoff_secs=backoff, error=%e, "RabbitMQ connection failed. Ghost Mode buffering...");
                        }
                    }
                    sleep(Duration::from_secs(backoff)).await;
                    backoff = std::cmp::min(backoff * 2, MAX_BACKOFF_SECS);
                }
            }
        });

        Self { buffer }
    }

    pub async fn publish(&self, routing_key: &str, payload: Value) {
        let mut b = self.buffer.lock().await;
        if b.len() >= MAX_BUFFER_CAPACITY {
            let _ = b.pop_front();
        }
        b.push_back((routing_key.to_string(), payload));
    }
}
