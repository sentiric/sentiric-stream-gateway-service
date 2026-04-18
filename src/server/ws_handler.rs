// File: src/server/ws_handler.rs
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::Response,
};
use prost::Message as ProstMessage;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use axum::extract::Query;
use sentiric_ai_pipeline_sdk::config::SdkConfig;
use sentiric_ai_pipeline_sdk::orchestrator::PipelineOrchestrator;
use sentiric_ai_pipeline_sdk::{PipelineEvent, PipelineInputEvent};
use sentiric_contracts::sentiric::event::v1::CognitiveMapUpdatedEvent;
use sentiric_contracts::sentiric::stream::v1::stream_session_request::Data as ReqData;
use sentiric_contracts::sentiric::stream::v1::stream_session_response::Data as RespData;
use sentiric_contracts::sentiric::stream::v1::{
    SessionConfig, StreamSessionRequest, StreamSessionResponse, TranscriptEvent, WordData,
};

use crate::app::AppState;

/// WSS Bağlantı Başlatıcı
pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let trace_id = params
        .get("trace_id")
        .cloned()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    ws.on_upgrade(move |socket| handle_websocket(socket, state, trace_id))
}

/// DTO: Oturum Context Verilerini taşımak için yapı
struct SessionContext {
    trace_id: String,
    span_id: String,
    tenant_id: String,
    session_id: String,
    user_id: String,
}

/// ANA YÖNETİCİ DÖNGÜ (Spagettiden arındırılmış ve Sahiplik Kurallarına Uyumlu)
pub async fn handle_websocket(
    mut socket: WebSocket,
    state: Arc<AppState>,
    initial_trace_id: String,
) {
    let span_id = Uuid::new_v4().to_string();
    let tenant_id = state.config.tenant_id.clone();

    // İlk mesaj (Config) beklenir
    let session_config = match wait_for_config(&mut socket, &initial_trace_id).await {
        Some(config) => config,
        None => {
            // [ARCH-COMPLIANCE FIX]: Sahiplik (Ownership) hatası çözüldü.
            // Kapatma işlemi referans üzerinden değil, soketin asıl sahibi tarafından yapılır.
            let _ = socket.close().await;
            return;
        }
    };

    let session_ctx = SessionContext {
        trace_id: if !session_config.trace_id.is_empty() {
            session_config.trace_id.clone()
        } else {
            initial_trace_id
        },
        span_id,
        tenant_id,
        session_id: if !session_config.session_id.is_empty() {
            session_config.session_id.clone()
        } else {
            Uuid::new_v4().to_string()
        },
        user_id: "stream-client".to_string(),
    };

    info!(
        event = "WS_CONNECTION_ESTABLISHED",
        trace_id = %session_ctx.trace_id, span_id = %session_ctx.span_id, tenant_id = %session_ctx.tenant_id, session_id = %session_ctx.session_id,
        "Session authenticated and ready."
    );

    // Call Started Olayını RMQ'ya bas
    publish_call_started(&state, &session_ctx).await;

    // AI Pipeline SDK Ayarları
    let sdk_config = build_sdk_config(&state.config, &session_config);

    let orchestrator = match PipelineOrchestrator::new(sdk_config).await {
        Ok(orch) => orch,
        Err(e) => {
            error!(event = "ORCHESTRATOR_INIT_FAIL", trace_id = %session_ctx.trace_id, error = %e, "Failed to init AI Pipeline.");
            let _ = socket.close().await;
            return;
        }
    };

    let (rx_input_tx, rx_input_rx) = mpsc::channel::<PipelineInputEvent>(128);
    let (tx_out_tx, mut tx_out_rx) = mpsc::channel(128);
    let (interrupt_tx, interrupt_rx) = mpsc::channel(10);

    // Orchestrator'u bağımsız task'a al
    spawn_orchestrator(
        orchestrator,
        &session_ctx,
        rx_input_rx,
        tx_out_tx,
        interrupt_rx,
    );

    let mut cognitive_rx = state.cognitive_tx.subscribe();

    // ⚡ ANA EVENT DÖNGÜSÜ (Yalnızca Helper Fonksiyonları Çağırır)
    loop {
        tokio::select! {
            ws_msg = socket.recv() => {
                if !process_client_message(ws_msg, &rx_input_tx, &interrupt_tx).await {
                    break; // İstemci koptu
                }
            }
            cog_event_res = cognitive_rx.recv() => {
                process_cognitive_map(cog_event_res, &session_ctx.trace_id, &mut socket).await;
            }
            ai_event = tx_out_rx.recv() => {
                if !process_ai_event(ai_event, &state, &session_ctx, &mut socket).await {
                    break; // Pipeline bitti veya koptu
                }
            }
        }
    }

    // Çağrı bitti, soketi güvenli kapat
    publish_call_ended(&state, &session_ctx).await;
    let _ = socket.close().await;
}

// ====================================================================================
// YARDIMCI FONKSİYONLAR (HELPER FUNCTIONS) - SOLID PRENSİPLERİ
// ====================================================================================

/// İlk mesaja bakar, eğer SessionConfig ise döner. Socket'i kapatma işlemi çağıran fonksiyona aittir.
async fn wait_for_config(socket: &mut WebSocket, trace_id: &str) -> Option<SessionConfig> {
    if let Some(Ok(Message::Binary(bin))) = socket.recv().await {
        match StreamSessionRequest::decode(&bin[..]) {
            Ok(req) => {
                if let Some(ReqData::Config(config)) = req.data {
                    return Some(config);
                } else {
                    error!(event = "INVALID_FIRST_MESSAGE", trace_id = %trace_id, "First message MUST be SessionConfig.");
                }
            }
            Err(e) => {
                error!(event = "PROTOBUF_DECODE_ERROR", trace_id = %trace_id, error = %e, "Failed to decode config.")
            }
        }
    } else {
        warn!(event = "WS_CONNECTION_DROPPED_EARLY", trace_id = %trace_id, "Client disconnected during handshake.");
    }
    None
}

/// İstemciden (WSS) gelen ses/metin mesajlarını Pipeline'a yönlendirir.
async fn process_client_message(
    ws_msg: Option<Result<Message, axum::Error>>,
    rx_input_tx: &mpsc::Sender<PipelineInputEvent>,
    interrupt_tx: &mpsc::Sender<()>,
) -> bool {
    match ws_msg {
        Some(Ok(Message::Binary(bin))) => {
            if let Ok(req) = StreamSessionRequest::decode(&bin[..]) {
                match req.data {
                    Some(ReqData::AudioChunk(chunk)) => {
                        let _ = rx_input_tx.send(PipelineInputEvent::Audio(chunk)).await;
                    }
                    Some(ReqData::TextMessage(text)) => {
                        let _ = rx_input_tx.send(PipelineInputEvent::Text(text)).await;
                    }
                    Some(ReqData::Control(ctrl)) => {
                        if ctrl.event == 1 {
                            let _ = interrupt_tx.try_send(());
                        } else if ctrl.event == 2 {
                            let _ = rx_input_tx.try_send(PipelineInputEvent::Audio(vec![]));
                        }
                    }
                    _ => {}
                }
            }
            true
        }
        Some(Ok(Message::Close(_))) | None => false,
        _ => true,
    }
}

/// AI Pipeline'dan çıkan sesi/metni alır, WebSockets ve RMQ'ya basar.
async fn process_ai_event(
    ai_event: Option<PipelineEvent>,
    state: &Arc<AppState>,
    ctx: &SessionContext,
    socket: &mut WebSocket,
) -> bool {
    match ai_event {
        Some(PipelineEvent::AcousticMoodShifted {
            session_id: evt_sess_id,
            previous_mood,
            current_mood,
            arousal_shift,
            valence_shift,
            speaker_id,
            speaker_vec,
        }) => {
            use sentiric_contracts::sentiric::event::v1::AcousticMoodShiftedEvent;
            let shift_event = AcousticMoodShiftedEvent {
                event_type: "acoustic.mood.shifted".to_string(),
                trace_id: ctx.trace_id.clone(),
                call_id: evt_sess_id,
                timestamp: Some(prost_types::Timestamp {
                    seconds: chrono::Utc::now().timestamp(),
                    nanos: chrono::Utc::now().timestamp_subsec_nanos() as i32,
                }),
                previous_mood,
                current_mood: current_mood.clone(),
                arousal_shift,
                valence_shift,
                speaker_id,
                speaker_vec,
            };

            let mut buf = Vec::new();
            if shift_event.encode(&mut buf).is_ok() {
                state
                    .ghost_publisher
                    .publish_protobuf("acoustic.mood.shifted", buf)
                    .await;
            }

            let status_json = json!({
                "type": "MOOD_SHIFT", "arousal_shift": arousal_shift, "new_mood": current_mood
            })
            .to_string();
            send_ws_response(socket, RespData::StatusUpdate(status_json)).await;
            true
        }
        Some(PipelineEvent::Audio(chunk)) => {
            send_ws_response(socket, RespData::AudioResponse(chunk)).await
        }
        Some(PipelineEvent::ClearBuffer) => {
            send_ws_response(socket, RespData::ClearAudioBuffer(true)).await
        }
        Some(PipelineEvent::Transcript(td)) => {
            let mapped_words: Vec<WordData> = td
                .words
                .into_iter()
                .map(|w| WordData {
                    word: w.word,
                    start: w.start,
                    end: w.end,
                    probability: w.probability,
                })
                .collect();

            let t_event = TranscriptEvent {
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
            };
            send_ws_response(socket, RespData::Transcript(t_event)).await
        }
        None => false,
    }
}

/// Crystalline'den broadcast edilen Zihin Haritalarını dinler ve doğru WebSocket'e yollar.
async fn process_cognitive_map(
    cog_event_res: Result<CognitiveMapUpdatedEvent, tokio::sync::broadcast::error::RecvError>,
    trace_id: &str,
    socket: &mut WebSocket,
) {
    if let Ok(cog_event) = cog_event_res {
        if cog_event.trace_id == trace_id {
            send_ws_response(socket, RespData::CognitiveMap(cog_event)).await;
        }
    }
}

/// Sockets'e veri basan jenerik yardımcı
async fn send_ws_response(socket: &mut WebSocket, data: RespData) -> bool {
    let resp = StreamSessionResponse { data: Some(data) };
    let mut buf = Vec::new();
    if resp.encode(&mut buf).is_ok() {
        return socket.send(Message::Binary(buf)).await.is_ok();
    }
    true
}

/// SDK Configuration Builder
fn build_sdk_config(app_cfg: &crate::config::AppConfig, sess_cfg: &SessionConfig) -> SdkConfig {
    let default_voice =
        std::env::var("TTS_DEFAULT_VOICE_ID").unwrap_or_else(|_| "omnivoice:female".to_string());

    SdkConfig {
        stt_gateway_url: app_cfg.stt_gateway_url.clone(),
        dialog_service_url: app_cfg.dialog_service_url.clone(),
        tts_gateway_url: app_cfg.tts_gateway_url.clone(),
        tls_ca_path: app_cfg.tls_ca_path.clone(),
        tls_cert_path: app_cfg.tls_cert_path.clone(),
        tls_key_path: app_cfg.tls_key_path.clone(),
        language_code: sess_cfg.language.clone(),
        system_prompt_id: if sess_cfg.system_prompt_id.is_empty() {
            "PROMPT_SYSTEM_DEFAULT".into()
        } else {
            sess_cfg.system_prompt_id.clone()
        },
        tts_voice_id: if sess_cfg.tts_voice_id.is_empty() {
            default_voice
        } else {
            sess_cfg.tts_voice_id.clone()
        },
        tts_sample_rate: if sess_cfg.sample_rate > 0 {
            sess_cfg.sample_rate
        } else {
            16000
        },
        edge_mode: sess_cfg.edge_mode,
        listen_only_mode: sess_cfg.listen_only_mode,
        speak_only_mode: sess_cfg.speak_only_mode,
        chat_only_mode: sess_cfg.chat_only_mode,
    }
}

/// AI Orchestrator Task Spawner
fn spawn_orchestrator(
    orchestrator: PipelineOrchestrator,
    ctx: &SessionContext,
    rx_input_rx: mpsc::Receiver<PipelineInputEvent>,
    tx_out_tx: mpsc::Sender<PipelineEvent>,
    interrupt_rx: mpsc::Receiver<()>,
) {
    let tr_id = ctx.trace_id.clone();
    let sp_id = ctx.span_id.clone();
    let ten_id = ctx.tenant_id.clone();
    let sess_id = ctx.session_id.clone();
    let usr_id = ctx.user_id.clone();

    tokio::spawn(async move {
        if let Err(e) = orchestrator
            .run_pipeline(
                sess_id,
                usr_id,
                tr_id.clone(),
                sp_id,
                ten_id,
                rx_input_rx,
                tx_out_tx,
                interrupt_rx,
            )
            .await
        {
            error!(event = "PIPELINE_ERROR", trace_id = %tr_id, error = %e, "Pipeline fatal error.");
        }
    });
}

/// Call Started Publisher
async fn publish_call_started(state: &Arc<AppState>, ctx: &SessionContext) {
    let call_started = sentiric_contracts::sentiric::event::v1::CallStartedEvent {
        event_type: "call.started".to_string(),
        trace_id: ctx.trace_id.clone(),
        call_id: ctx.session_id.clone(),
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
}

/// Call Ended Publisher
async fn publish_call_ended(state: &Arc<AppState>, ctx: &SessionContext) {
    let call_ended = sentiric_contracts::sentiric::event::v1::CallEndedEvent {
        event_type: "call.ended".to_string(),
        trace_id: ctx.trace_id.clone(),
        call_id: ctx.session_id.clone(),
        timestamp: Some(prost_types::Timestamp {
            seconds: chrono::Utc::now().timestamp(),
            nanos: chrono::Utc::now().timestamp_subsec_nanos() as i32,
        }),
        reason: "client_disconnected".to_string(),
    };
    let mut buf = Vec::new();
    if call_ended.encode(&mut buf).is_ok() {
        state
            .ghost_publisher
            .publish_protobuf("call.ended", buf)
            .await;
    }
    tracing::info!(event="WS_SESSION_CLOSED", trace_id=%ctx.trace_id, session_id=%ctx.session_id, "WebSocket session safely closed.");
}
