use crate::app::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use prost::Message as ProstMessage;
use sentiric_ai_pipeline_sdk::{PipelineOrchestrator, SdkConfig};
use sentiric_contracts::sentiric::stream::v1::stream_session_request::Data as StreamReqData;
use sentiric_contracts::sentiric::stream::v1::stream_session_response::Data as StreamResData;
use sentiric_contracts::sentiric::stream::v1::{StreamSessionRequest, StreamSessionResponse};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let session_id = Uuid::new_v4().to_string();
    let trace_id = Uuid::new_v4().to_string();
    let span_id = Uuid::new_v4().to_string();
    let tenant_id = state.config.tenant_id.clone();

    info!(
        event = "WS_CLIENT_CONNECTED",
        trace_id = %trace_id,
        span_id = %span_id,
        tenant_id = %tenant_id,
        "New WebSocket client connected."
    );

    // 1. Initial Handshake (Config)
    let config_msg = match socket.next().await {
        Some(Ok(Message::Binary(bytes))) => match StreamSessionRequest::decode(bytes.as_slice()) {
            Ok(req) => req,
            Err(e) => {
                warn!(
                    event = "WS_DECODE_FAIL",
                    trace_id = %trace_id,
                    error = %e,
                    "Handshake decode failed"
                );
                let _ = socket.close().await;
                return;
            }
        },
        _ => {
            let _ = socket.close().await;
            return;
        }
    };

    let session_config = match config_msg.data {
        Some(StreamReqData::Config(cfg)) => cfg,
        _ => {
            warn!(
                event = "WS_MISSING_CONFIG",
                trace_id = %trace_id,
                "First message must be config"
            );
            let _ = socket.close().await;
            return;
        }
    };

    // 2. Publish Session Started Event (Ghost Mode Supported)
    state
        .ghost_publisher
        .publish(
            "stream.session.started",
            json!({
                "event_type": "stream.session.started",
                "trace_id": trace_id,
                "tenant_id": tenant_id,
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "payload": {
                    "session_id": session_id,
                    "user_id": session_config.token,
                    "edge_mode": session_config.edge_mode
                }
            }),
        )
        .await;

    // 3. SDK Orchestration
    let sdk_config = SdkConfig {
        stt_gateway_url: state.config.stt_gateway_url.clone(),
        dialog_service_url: state.config.dialog_service_url.clone(),
        tts_gateway_url: state.config.tts_gateway_url.clone(),
        tls_ca_path: state.config.tls_ca_path.clone(),
        tls_cert_path: state.config.tls_cert_path.clone(),
        tls_key_path: state.config.tls_key_path.clone(),
        language_code: session_config.language,
        system_prompt_id: "default".to_string(),
        tts_voice_id: "default".to_string(),
        tts_sample_rate: session_config.sample_rate,
    };

    let orchestrator = match PipelineOrchestrator::new(sdk_config).await {
        Ok(orch) => orch,
        Err(e) => {
            error!(
                event = "SDK_INIT_FAIL",
                trace_id = %trace_id,
                error = %e,
                "SDK initialization failed"
            );
            let _ = socket.close().await;
            return;
        }
    };

    let (tx_sdk_in, rx_sdk_in) = mpsc::channel::<Vec<u8>>(128);
    let (tx_sdk_out, mut rx_sdk_out) = mpsc::channel::<Vec<u8>>(128);

    let orch_trace_id = trace_id.clone();
    let orch_span_id = span_id.clone();
    let orch_tenant_id = tenant_id.clone();
    let user_id = session_config.token.clone();

    // Spawn AI SDK Pipeline
    tokio::spawn(async move {
        if let Err(e) = orchestrator
            .run_pipeline(
                session_id,
                user_id,
                orch_trace_id.clone(),
                orch_span_id,
                orch_tenant_id,
                rx_sdk_in,
                tx_sdk_out,
            )
            .await
        {
            error!(
                event = "SDK_PIPELINE_ERROR",
                trace_id = %orch_trace_id,
                error = %e,
                "AI Pipeline execution error"
            );
        }
    });

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // 4. TX TASK (SDK Audio -> WebSocket Client)
    let tx_trace_id = trace_id.clone();
    let mut tx_task = tokio::spawn(async move {
        while let Some(audio_chunk) = rx_sdk_out.recv().await {
            let resp = StreamSessionResponse {
                data: Some(StreamResData::AudioResponse(audio_chunk)),
            };
            let mut buf = Vec::new();

            // [CLIPPY FIX]: Collapsed nested if for cleaner performance
            if resp.encode(&mut buf).is_ok() && ws_sender.send(Message::Binary(buf)).await.is_err()
            {
                break;
            }
        }
        warn!(event = "WS_TX_CLOSED", trace_id = %tx_trace_id, "Outgoing audio stream task ended");
    });

    // 5. RX TASK (WebSocket Audio -> SDK Input)
    let rx_trace_id = trace_id.clone();
    let mut rx_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            if let Message::Binary(bytes) = msg {
                if let Ok(req) = StreamSessionRequest::decode(bytes.as_slice()) {
                    if let Some(StreamReqData::AudioChunk(chunk)) = req.data {
                        if tx_sdk_in.send(chunk).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
        info!(event = "WS_RX_CLOSED", trace_id = %rx_trace_id, "Incoming audio stream task ended");
    });

    // Bridge Logic: Select the first task to fail or finish
    tokio::select! {
        _ = (&mut tx_task) => rx_task.abort(),
        _ = (&mut rx_task) => tx_task.abort(),
    }

    // 6. Finalize: Publish Ended Event
    state
        .ghost_publisher
        .publish(
            "stream.session.ended",
            json!({
                "event_type": "stream.session.ended",
                "trace_id": trace_id,
                "tenant_id": tenant_id,
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "payload": { "reason": "session_terminated" }
            }),
        )
        .await;
}
