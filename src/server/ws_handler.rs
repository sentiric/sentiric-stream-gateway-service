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
use uuid::Uuid; // [YENİ] RMQ Event payload'u için

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

    let mut trace_id = Uuid::new_v4().to_string();
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
    let mut listen_only = false; // [YENİ]
    let mut lang_code = "tr-TR".to_string();
    let mut sample_rate = 16000;

    if let Some(Ok(msg)) = socket.recv().await {
        if let Message::Binary(bin) = msg {
            match StreamSessionRequest::decode(&bin[..]) {
                Ok(req) => {
                    if let Some(Data::Config(session_config)) = req.data {
                        edge_mode_active = session_config.edge_mode;
                        listen_only = session_config.listen_only_mode; // [YENİ]
                        lang_code = session_config.language;

                        if session_config.sample_rate > 0 {
                            sample_rate = session_config.sample_rate;
                        }

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
                            "Session configuration verified and accepted."
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
        listen_only_mode: listen_only, // [YENİ]
    };

    let orchestrator = match PipelineOrchestrator::new(sdk_config).await {
        Ok(orch) => orch,
        Err(e) => {
            error!(event = "ORCHESTRATOR_INIT_FAIL", trace_id = %trace_id, error = %e, "Failed to init AI Pipeline.");
            let _ = socket.close().await;
            return;
        }
    };

    let (rx_audio_tx, rx_audio_rx) = mpsc::channel(128);
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
                rx_audio_rx,
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
                                    if rx_audio_tx.send(chunk).await.is_err() { break; }
                                }
                                Some(Data::Control(ctrl)) => {
                                    if ctrl.event == 1 { // EVENT_TYPE_INTERRUPT (Barge-in)
                                        let _ = interrupt_tx.try_send(());
                                    } else if ctrl.event == 2 { // [YENİ] EVENT_TYPE_EOS (End of Speech)
                                        // Cümle bitti, STT tamponunu zorla kapat ve Finalize et!
                                        let _ = rx_audio_tx.try_send(vec![]);
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
                    // [YENİ]: Deep Waters Event Yakalama ve RMQ'ya Fırlatma
                    Some(sentiric_ai_pipeline_sdk::PipelineEvent::AcousticMoodShifted { previous_mood, current_mood, arousal_shift, valence_shift, speaker_id }) => {
                        let payload = json!({
                            "trace_id": loop_tr_id,
                            "session_id": session_id,
                            "previous_mood": previous_mood,
                            "current_mood": current_mood,
                            "arousal_shift": arousal_shift,
                            "valence_shift": valence_shift,
                            "speaker_id": speaker_id
                        });
                        // Bu event, Crystalline servisi veya SRE analizleri için RabbitMQ'ya basılır.
                        state.ghost_publisher.publish("acoustic.mood.shifted", payload).await;

                        // [EKLENDİ]: İsteğe bağlı olarak Web UI tarafında da çizdirmek istersen
                        // bunu Status Update olarak socket üzerinden gönderebiliriz.
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

                        // Kelimeleri dönüştür
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
}
