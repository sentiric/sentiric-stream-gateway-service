use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::Response,
};
use prost::Message as ProstMessage;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use sentiric_ai_pipeline_sdk::config::SdkConfig;
use sentiric_ai_pipeline_sdk::orchestrator::PipelineOrchestrator;
use sentiric_contracts::sentiric::stream::v1::stream_session_request::Data;
use sentiric_contracts::sentiric::stream::v1::StreamSessionRequest;

use crate::app::AppState;

pub async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_websocket(socket, state))
}

pub async fn handle_websocket(mut socket: WebSocket, state: Arc<AppState>) {
    let config = &state.config;

    // SUTS v4.0 Context Generation
    let trace_id = Uuid::new_v4().to_string();
    let span_id = Uuid::new_v4().to_string();
    let tenant_id = "default-tenant".to_string();
    let session_id = Uuid::new_v4().to_string();
    let user_id = "stream-client".to_string();

    info!(
        event = "WS_CONNECTION_ESTABLISHED",
        trace_id = %trace_id,
        span_id = %span_id,
        tenant_id = %tenant_id,
        "New WebSocket connection established for Stream Gateway."
    );

    let mut edge_mode_active = false;
    let mut lang_code = "tr-TR".to_string();

    // Handshake
    if let Some(Ok(msg)) = socket.recv().await {
        if let Message::Binary(bin) = msg {
            match StreamSessionRequest::decode(&bin[..]) {
                Ok(req) => {
                    if let Some(Data::Config(session_config)) = req.data {
                        edge_mode_active = session_config.edge_mode;
                        lang_code = session_config.language;

                        info!(
                            event = "SESSION_CONFIG_RECEIVED",
                            trace_id = %trace_id,
                            span_id = %span_id,
                            tenant_id = %tenant_id,
                            edge_mode = edge_mode_active,
                            language = %lang_code,
                            "Session configuration verified and accepted."
                        );
                    } else {
                        error!(
                            event = "INVALID_FIRST_MESSAGE",
                            trace_id = %trace_id,
                            span_id = %span_id,
                            tenant_id = %tenant_id,
                            "Protocol Violation: First message MUST be SessionConfig. Dropping connection."
                        );
                        let _ = socket.close().await;
                        return;
                    }
                }
                Err(e) => {
                    error!(
                        event = "PROTOBUF_DECODE_ERROR",
                        trace_id = %trace_id,
                        span_id = %span_id,
                        tenant_id = %tenant_id,
                        error = %e,
                        "Failed to decode StreamSessionRequest."
                    );
                    let _ = socket.close().await;
                    return;
                }
            }
        }
    } else {
        warn!(
            event = "WS_CONNECTION_DROPPED_EARLY",
            trace_id = %trace_id,
            span_id = %span_id,
            tenant_id = %tenant_id,
            "Client disconnected before sending SessionConfig."
        );
        return;
    }

    let sdk_config = SdkConfig {
        stt_gateway_url: config.stt_gateway_url.clone(),
        dialog_service_url: config.dialog_service_url.clone(),
        tts_gateway_url: config.tts_gateway_url.clone(),
        tls_ca_path: config.tls_ca_path.clone(),
        tls_cert_path: config.tls_cert_path.clone(),
        tls_key_path: config.tls_key_path.clone(),
        language_code: lang_code,
        system_prompt_id: "default-stream-prompt".to_string(),
        tts_voice_id: "coqui:default".to_string(),
        tts_sample_rate: 24000,
        edge_mode: edge_mode_active,
    };

    let orchestrator = match PipelineOrchestrator::new(sdk_config).await {
        Ok(orch) => orch,
        Err(e) => {
            error!(
                event = "ORCHESTRATOR_INIT_FAIL",
                trace_id = %trace_id,
                span_id = %span_id,
                tenant_id = %tenant_id,
                error = %e,
                "Failed to initialize AI Pipeline Orchestrator."
            );
            let _ = socket.close().await;
            return;
        }
    };

    let (rx_audio_tx, rx_audio_rx) = mpsc::channel(128);
    let (tx_audio_tx, mut tx_audio_rx) = mpsc::channel(128);

    let tr_id = trace_id.clone();
    let sp_id = span_id.clone();
    let ten_id = tenant_id.clone();

    tokio::spawn(async move {
        if let Err(e) = orchestrator
            .run_pipeline(
                session_id,
                user_id,
                tr_id.clone(),
                sp_id.clone(),
                ten_id.clone(),
                rx_audio_rx,
                tx_audio_tx,
            )
            .await
        {
            error!(
                event = "PIPELINE_ERROR",
                trace_id = %tr_id,
                span_id = %sp_id,
                tenant_id = %ten_id,
                error = %e,
                "Pipeline orchestrator encountered a fatal error."
            );
        }
    });

    loop {
        tokio::select! {
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(Message::Binary(bin))) => {
                        if let Ok(req) = StreamSessionRequest::decode(&bin[..]) {
                            if let Some(Data::AudioChunk(chunk)) = req.data {
                                if rx_audio_tx.send(chunk).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!(
                            event = "WS_CLIENT_DISCONNECTED",
                            trace_id = %trace_id,
                            span_id = %span_id,
                            tenant_id = %tenant_id,
                            "Client closed the websocket connection."
                        );
                        break;
                    }
                    _ => {}
                }
            }

            ai_audio = tx_audio_rx.recv() => {
                match ai_audio {
                    Some(chunk) => {
                        use sentiric_contracts::sentiric::stream::v1::stream_session_response::Data as RespData;
                        use sentiric_contracts::sentiric::stream::v1::StreamSessionResponse;

                        let resp = StreamSessionResponse {
                            data: Some(RespData::AudioResponse(chunk))
                        };

                        let mut buf = Vec::new();
                        // [FIX] Clippy Collapsible If: İki if koşulu && ile tek satıra indirgendi
                        if resp.encode(&mut buf).is_ok()
                            && socket.send(Message::Binary(buf)).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
}
