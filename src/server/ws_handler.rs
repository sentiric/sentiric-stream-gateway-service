// File: src/server/ws_handler.rs
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::Response,
};
use prost::Message as ProstMessage;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use sentiric_ai_pipeline_sdk::config::SdkConfig;
use sentiric_ai_pipeline_sdk::orchestrator::PipelineOrchestrator;
use sentiric_ai_pipeline_sdk::PipelineInputEvent;
use sentiric_contracts::sentiric::stream::v1::stream_session_request::Data;
use sentiric_contracts::sentiric::stream::v1::StreamSessionRequest;

use crate::app::AppState;

use axum::extract::Query; // [YENİ]
use std::collections::HashMap; // [YENİ]

pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>, // [YENİ]: URL parametrelerini oku
) -> Response {
    // Eğer URL'de trace_id varsa onu kullan, yoksa yeni üret
    let trace_id = params
        .get("trace_id")
        .cloned()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    ws.on_upgrade(move |socket| handle_websocket(socket, state, trace_id))
}

pub async fn handle_websocket(
    mut socket: WebSocket,
    state: Arc<AppState>,
    initial_trace_id: String,
) {
    let mut trace_id = initial_trace_id; // Artık dışarıdan geliyor
    let config = &state.config;

    // let mut trace_id = Uuid::new_v4().to_string();
    let span_id = Uuid::new_v4().to_string();
    let tenant_id = config.tenant_id.clone();
    let mut session_id = Uuid::new_v4().to_string();
    let user_id = "stream-client".to_string();

    info!(
        event = "WS_CONNECTION_ESTABLISHED",
        trace_id = %trace_id, span_id = %span_id, tenant_id = %tenant_id,
        "New WebSocket connection established for Stream Gateway."
    );

    let mut edge_mode_active = false;
    let mut listen_only = false;
    let mut speak_only = false;
    let mut chat_only = false;
    let mut lang_code = "tr-TR".to_string();
    let mut sample_rate = 16000;

    if let Some(Ok(msg)) = socket.recv().await {
        if let Message::Binary(bin) = msg {
            match StreamSessionRequest::decode(&bin[..]) {
                Ok(req) => {
                    if let Some(Data::Config(session_config)) = req.data {
                        edge_mode_active = session_config.edge_mode;
                        listen_only = session_config.listen_only_mode;
                        speak_only = session_config.speak_only_mode;
                        chat_only = session_config.chat_only_mode;
                        lang_code = session_config.language;

                        if session_config.sample_rate > 0 {
                            sample_rate = session_config.sample_rate;
                        }

                        // [ARCH-COMPLIANCE FIX]: Trace ID Merge
                        // İlk bağlantıda üretilen geçici ID'yi loglayıp, yeni ID'ye (SDK'dan gelen) geçiş yaptığımızı belirtiyoruz.
                        let temp_trace_id = trace_id.clone();

                        if !session_config.trace_id.is_empty() {
                            trace_id = session_config.trace_id.clone();
                        }
                        if !session_config.session_id.is_empty() {
                            session_id = session_config.session_id.clone();
                        }

                        info!(
                            event = "SESSION_CONFIG_RECEIVED",
                            trace_id = %trace_id, span_id = %span_id, tenant_id = %tenant_id,
                            edge_mode = edge_mode_active, listen_only = listen_only, language = %lang_code,
                            sample_rate = sample_rate, session_id = %session_id,
                            temp_trace_id = %temp_trace_id, // [YENİ]: Adli bilişim (Forensics) için geçici ID'yi loga ekledik
                            "Session configuration verified and accepted. Trace bounds merged."
                        );
                    } else {
                        error!(event = "INVALID_FIRST_MESSAGE", trace_id = %trace_id, "Protocol Violation: First message MUST be SessionConfig.");
                        let _ = socket.close().await;
                        return;
                    }
                }
                Err(e) => {
                    error!(event = "PROTOBUF_DECODE_ERROR", trace_id = %trace_id, error = %e, "Failed to decode StreamSessionRequest.");
                    let _ = socket.close().await;
                    return;
                }
            }
        }
    } else {
        warn!(event = "WS_CONNECTION_DROPPED_EARLY", trace_id = %trace_id, "Client disconnected early.");
        return;
    }

    let call_started = sentiric_contracts::sentiric::event::v1::CallStartedEvent {
        event_type: "call.started".to_string(),
        trace_id: trace_id.clone(),
        call_id: session_id.clone(),
        from_uri: "web-client".to_string(),
        to_uri: "ai-pipeline".to_string(),
        timestamp: Some(prost_types::Timestamp {
            seconds: chrono::Utc::now().timestamp(),
            nanos: chrono::Utc::now().timestamp_subsec_nanos() as i32,
        }),
        dialplan_resolution: None,
        media_info: Some(sentiric_contracts::sentiric::event::v1::MediaInfo {
            caller_rtp_addr: "websocket".to_string(),
            server_rtp_port: 0,
        }),
    };

    let mut buf = Vec::new();
    if call_started.encode(&mut buf).is_ok() {
        state
            .ghost_publisher
            .publish_protobuf("call.started", buf)
            .await;
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
        tts_sample_rate: sample_rate,
        edge_mode: edge_mode_active,
        listen_only_mode: listen_only,
        speak_only_mode: speak_only,
        chat_only_mode: chat_only,
    };

    let orchestrator = match PipelineOrchestrator::new(sdk_config).await {
        Ok(orch) => orch,
        Err(e) => {
            error!(event = "ORCHESTRATOR_INIT_FAIL", trace_id = %trace_id, error = %e, "Failed to init AI Pipeline.");
            let _ = socket.close().await;
            return;
        }
    };

    let (rx_input_tx, rx_input_rx) = mpsc::channel::<PipelineInputEvent>(128);
    let (tx_out_tx, mut tx_out_rx) = mpsc::channel(128);
    let (interrupt_tx, interrupt_rx) = mpsc::channel(10);

    let tr_id = trace_id.clone();
    let sp_id = span_id.clone();
    let ten_id = tenant_id.clone();
    let sess_id = session_id.clone();

    tokio::spawn(async move {
        if let Err(e) = orchestrator
            .run_pipeline(
                sess_id,
                user_id,
                tr_id.clone(),
                sp_id.clone(),
                ten_id.clone(),
                rx_input_rx,
                tx_out_tx,
                interrupt_rx,
            )
            .await
        {
            error!(event = "PIPELINE_ERROR", trace_id = %tr_id, error = %e, "Pipeline fatal error.");
        }
    });

    let loop_tr_id = trace_id.clone();

    loop {
        tokio::select! {
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(Message::Binary(bin))) => {
                        if let Ok(req) = StreamSessionRequest::decode(&bin[..]) {
                            match req.data {
                                Some(Data::AudioChunk(chunk)) => {
                                    if rx_input_tx.send(PipelineInputEvent::Audio(chunk)).await.is_err() { break; }
                                }
                                Some(Data::TextMessage(text)) => {
                                    if rx_input_tx.send(PipelineInputEvent::Text(text)).await.is_err() { break; }
                                }
                                Some(Data::Control(ctrl)) => {
                                    if ctrl.event == 1 {
                                        let _ = interrupt_tx.try_send(());
                                    } else if ctrl.event == 2 {
                                        let _ = rx_input_tx.try_send(PipelineInputEvent::Audio(vec![]));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            ai_event = tx_out_rx.recv() => {
                match ai_event {
                    // [ARCH-COMPLIANCE FIX]: SDK v0.1.16 session_id eklendi
                    Some(sentiric_ai_pipeline_sdk::PipelineEvent::AcousticMoodShifted { session_id: evt_sess_id, previous_mood, current_mood, arousal_shift, valence_shift, speaker_id }) => {
                        let payload = json!({
                            "trace_id": loop_tr_id,
                            "session_id": evt_sess_id,
                            "previous_mood": previous_mood,
                            "current_mood": current_mood,
                            "arousal_shift": arousal_shift,
                            "valence_shift": valence_shift,
                            "speaker_id": speaker_id
                        });

                        state.ghost_publisher.publish_json("acoustic.mood.shifted", payload).await;

                        use sentiric_contracts::sentiric::stream::v1::stream_session_response::Data as RespData;
                        use sentiric_contracts::sentiric::stream::v1::StreamSessionResponse;
                        let status_json = json!({
                            "type": "MOOD_SHIFT",
                            "arousal_shift": arousal_shift,
                            "new_mood": current_mood
                        }).to_string();
                        let resp = StreamSessionResponse { data: Some(RespData::StatusUpdate(status_json)) };
                        let mut buf = Vec::new();
                        if resp.encode(&mut buf).is_ok() {
                            let _ = socket.send(Message::Binary(buf)).await;
                        }
                    }
                    Some(sentiric_ai_pipeline_sdk::PipelineEvent::Audio(chunk)) => {
                        use sentiric_contracts::sentiric::stream::v1::stream_session_response::Data as RespData;
                        use sentiric_contracts::sentiric::stream::v1::StreamSessionResponse;
                        let resp = StreamSessionResponse { data: Some(RespData::AudioResponse(chunk)) };
                        let mut buf = Vec::new();
                        if resp.encode(&mut buf).is_ok() && socket.send(Message::Binary(buf)).await.is_err() { break; }
                    }
                    Some(sentiric_ai_pipeline_sdk::PipelineEvent::ClearBuffer) => {
                        use sentiric_contracts::sentiric::stream::v1::stream_session_response::Data as RespData;
                        use sentiric_contracts::sentiric::stream::v1::StreamSessionResponse;
                        let resp = StreamSessionResponse { data: Some(RespData::ClearAudioBuffer(true)) };
                        let mut buf = Vec::new();
                        if resp.encode(&mut buf).is_ok() && socket.send(Message::Binary(buf)).await.is_err() { break; }
                    }
                    Some(sentiric_ai_pipeline_sdk::PipelineEvent::Transcript(td)) => {
                        use sentiric_contracts::sentiric::stream::v1::stream_session_response::Data as RespData;
                        use sentiric_contracts::sentiric::stream::v1::{StreamSessionResponse, TranscriptEvent, WordData};

                        let mapped_words: Vec<WordData> = td.words.into_iter().map(|w| WordData {
                            word: w.word,
                            start: w.start,
                            end: w.end,
                            probability: w.probability,
                        }).collect();

                        let resp = StreamSessionResponse {
                            data: Some(RespData::Transcript(TranscriptEvent {
                                text: td.text,
                                is_final: td.is_final,
                                sender: td.sender,
                                emotion: td.emotion,
                                gender: td.gender,
                                arousal: td.arousal,
                                valence: td.valence,
                                speaker_id: td.speaker_id,
                                speaker_vec: td.speaker_vec,
                                words: mapped_words,
                            }))
                        };
                        let mut buf = Vec::new();
                        if resp.encode(&mut buf).is_ok() && socket.send(Message::Binary(buf)).await.is_err() { break; }
                    }
                    None => break,
                }
            }
        }
    }

    let call_ended = sentiric_contracts::sentiric::event::v1::CallEndedEvent {
        event_type: "call.ended".to_string(),
        trace_id: trace_id.clone(),
        call_id: session_id.clone(),
        timestamp: Some(prost_types::Timestamp {
            seconds: chrono::Utc::now().timestamp(),
            nanos: chrono::Utc::now().timestamp_subsec_nanos() as i32,
        }),
        reason: "client_disconnected".to_string(),
    };

    let mut end_buf = Vec::new();
    if call_ended.encode(&mut end_buf).is_ok() {
        state
            .ghost_publisher
            .publish_protobuf("call.ended", end_buf)
            .await;
    }

    tracing::info!(event="WS_SESSION_CLOSED", trace_id=%trace_id, session_id=%session_id, "WebSocket session safely closed and billed.");
}
