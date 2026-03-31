use lapin::{options::*, BasicProperties, Connection, ConnectionProperties};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct GhostPublisher {
    buffer: Arc<Mutex<VecDeque<(String, Value)>>>,
}

impl GhostPublisher {
    pub fn new(rabbitmq_url: String, tenant_id: String) -> Self {
        // Hata Düzeltme: VecDeque tipini açıkça belirtiyoruz (str size hatasını engellemek için)
        let buffer: Arc<Mutex<VecDeque<(String, Value)>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(1000)));
        let buffer_clone = buffer.clone();

        tokio::spawn(async move {
            if rabbitmq_url.is_empty() {
                warn!(event="MQ_DISABLED", tenant_id=%tenant_id, "Ghost Mode Active: RabbitMQ URL not provided. Events will be drained in memory.");
                loop {
                    sleep(Duration::from_secs(30)).await;
                    let mut b = buffer_clone.lock().await;
                    b.clear();
                }
            }

            loop {
                match Connection::connect(&rabbitmq_url, ConnectionProperties::default()).await {
                    Ok(conn) => {
                        info!(event="MQ_CONNECTED", tenant_id=%tenant_id, "Connected to RabbitMQ. Draining buffer...");
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
                                        buffer_clone
                                            .lock()
                                            .await
                                            .push_front((routing_key, payload));
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
                        error!(event="MQ_CONNECT_FAIL", tenant_id=%tenant_id, error=%e, "RabbitMQ connection failed. Ghost Mode buffering...");
                    }
                }
                sleep(Duration::from_secs(10)).await;
            }
        });

        Self { buffer }
    }

    pub async fn publish(&self, routing_key: &str, payload: Value) {
        let mut b = self.buffer.lock().await;
        if b.len() >= 1000 {
            let _ = b.pop_front();
        }
        b.push_back((routing_key.to_string(), payload));
    }
}
