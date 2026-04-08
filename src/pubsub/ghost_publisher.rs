// [ARCH-COMPLIANCE] SUTS v4.0 & Ghost Publisher (No-Panic)
#![allow(dead_code)]
use lapin::{options::*, BasicProperties, Connection, ConnectionProperties};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

const MAX_BUFFER_CAPACITY: usize = 1000;
const MAX_BACKOFF_SECS: u64 = 60;

// [ARCH-COMPLIANCE FIX]: Type Complexity hatasını gidermek için Type Alias
type RmqPayload = (String, String, Vec<u8>);
type SharedGhostBuffer = Arc<Mutex<VecDeque<RmqPayload>>>;

#[derive(Clone)]
pub struct GhostPublisher {
    buffer: SharedGhostBuffer,
}

impl GhostPublisher {
    pub fn new(rabbitmq_url: String, tenant_id: String) -> Self {
        let buffer: SharedGhostBuffer =
            Arc::new(Mutex::new(VecDeque::with_capacity(MAX_BUFFER_CAPACITY)));
        let buffer_clone = buffer.clone();

        tokio::spawn(async move {
            let clean_url = rabbitmq_url.trim().replace("\"", "");

            if clean_url.is_empty() {
                warn!(event="MQ_DISABLED", tenant_id=%tenant_id, "Ghost Mode Active: RabbitMQ URL not provided. Events drained in memory.");
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

                                    if let Some((routing_key, content_type, payload_bytes)) = msg {
                                        let res = channel
                                            .basic_publish(
                                                "sentiric_events",
                                                &routing_key,
                                                BasicPublishOptions::default(),
                                                &payload_bytes,
                                                BasicProperties::default()
                                                    .with_content_type(content_type.clone().into()),
                                            )
                                            .await;

                                        if res.is_err() {
                                            let mut b = buffer_clone.lock().await;
                                            if b.len() >= MAX_BUFFER_CAPACITY {
                                                warn!(event="GHOST_BUFFER_FULL_DISCARD", tenant_id=%tenant_id, "Ring buffer full. Dropping oldest message.");
                                            } else {
                                                b.push_front((
                                                    routing_key,
                                                    content_type,
                                                    payload_bytes,
                                                ));
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

    pub async fn publish_json(&self, routing_key: &str, payload: Value) {
        let bytes = serde_json::to_vec(&payload).unwrap_or_default();
        self.enqueue(routing_key, "application/json", bytes).await;
    }

    pub async fn publish_protobuf(&self, routing_key: &str, payload: Vec<u8>) {
        self.enqueue(routing_key, "application/protobuf", payload)
            .await;
    }

    async fn enqueue(&self, routing_key: &str, content_type: &str, payload: Vec<u8>) {
        let mut b = self.buffer.lock().await;
        if b.len() >= MAX_BUFFER_CAPACITY {
            let _ = b.pop_front();
        }
        b.push_back((routing_key.to_string(), content_type.to_string(), payload));
    }
}
