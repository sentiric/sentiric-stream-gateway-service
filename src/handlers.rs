use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, error, warn};
use uuid::Uuid;
use metrics::{counter, increment_gauge, decrement_gauge};
use crate::clients::GrpcClients;
use tonic::metadata::MetadataValue;
use std::str::FromStr;

// Contracts
use sentiric_contracts::sentiric::{
    stt::v1::{TranscribeStreamRequest},
    dialog::v1::{
        StreamConversationRequest, 
        stream_conversation_request, stream_conversation_response,
        ConversationConfig
    },
    tts::v1::{SynthesizeStreamRequest, TextType},
};

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(clients): State<Arc<GrpcClients>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, clients))
}

fn create_telemetry(phase: &str, status: &str, detail: &str) -> Message {
    let json = serde_json::json!({
        "type": "telemetry",
        "phase": phase,
        "status": status,
        "detail": detail,
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    Message::Text(json.to_string())
}

// YENİ: Cümle Tamponlama Yardımcısı
struct SentenceBuffer {
    buffer: String,
}

impl SentenceBuffer {
    fn new() -> Self {
        Self { buffer: String::new() }
    }

    // Metni ekler ve tamamlanmış cümleleri döndürür
    fn push_and_extract(&mut self, text: &str) -> Option<String> {
        self.buffer.push_str(text);
        
        // Basit noktalama kontrolü (Daha gelişmiş NLP gerekebilir ama şimdilik yeterli)
        // Eğer cümlenin sonunda noktalama varsa, o ana kadarki kısmı kesip döndür.
        if let Some(idx) = self.buffer.rfind(|c| ".?!:;".contains(c)) {
            // İndeks + 1 yaparak noktalama işaretini de dahil ediyoruz
            let complete_sentence = self.buffer[..=idx].to_string();
            // Kalan kısmı buffer'da tut
            self.buffer = self.buffer[idx+1..].to_string();
            
            // Eğer cümle çok kısaysa (örn: "A.") bir sonrakiyle birleşmesini bekle (opsiyonel)
            if complete_sentence.trim().len() < 2 {
                // Geri al (Basitlik için şimdilik direk gönderiyoruz)
                // self.buffer = complete_sentence + &self.buffer;
                // return None;
            }
            
            return Some(complete_sentence);
        }
        
        // Eğer buffer çok şişerse (noktalama gelmezse) zorla gönder (Latency koruması)
        if self.buffer.len() > 200 {
             let chunk = self.buffer.clone();
             self.buffer.clear();
             return Some(chunk);
        }

        None
    }
    
    // Kalan son parçayı al
    fn flush(&mut self) -> Option<String> {
        if self.buffer.trim().is_empty() {
            return None;
        }
        let chunk = self.buffer.clone();
        self.buffer.clear();
        Some(chunk)
    }
}

async fn handle_socket(socket: WebSocket, clients: Arc<GrpcClients>) {
    let session_id = Uuid::new_v4().to_string();
    info!("📱 New Stream Connection. Session: {}", session_id);
    
    increment_gauge!("stream_gateway_active_sessions", 1.0);
    counter!("stream_gateway_connections_total", 1);

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let voice_id = clients.default_voice_id.clone();

    // Kanallar
    let (tx_stt_in, rx_stt_in) = mpsc::channel::<TranscribeStreamRequest>(100);
    let (tx_dialog_in, rx_dialog_in) = mpsc::channel::<StreamConversationRequest>(100);
    let (tx_ws_out, mut rx_ws_out) = mpsc::channel::<Message>(100);

    let _ = ws_sender.send(create_telemetry("gateway", "connected", &format!("Session: {}", session_id))).await;

    // --- 1. STT LOOP ---
    let mut stt_client = clients.stt.clone();
    let tx_dialog_in_clone = tx_dialog_in.clone();
    let tx_ws_out_stt = tx_ws_out.clone();
    let session_id_clone = session_id.clone();

    let stt_handle = tokio::spawn(async move {
        let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx_stt_in);
        
        let mut request = tonic::Request::new(request_stream);
        if let Ok(meta_val) = MetadataValue::from_str(&session_id_clone) {
            request.metadata_mut().insert("x-trace-id", meta_val);
        }

        match stt_client.transcribe_stream(request).await {
            Ok(response) => {
                let mut inbound = response.into_inner();
                while let Some(result) = inbound.next().await {
                    match result {
                        Ok(stt_resp) => {
                            if !stt_resp.partial_transcription.is_empty() {
                                if stt_resp.is_final {
                                    info!("[{}] STT Final: {}", session_id_clone, stt_resp.partial_transcription);
                                    let _ = tx_ws_out_stt.send(create_telemetry("stt", "final", &stt_resp.partial_transcription)).await;
                                    
                                    let dialog_req = StreamConversationRequest {
                                        payload: Some(stream_conversation_request::Payload::TextInput(stt_resp.partial_transcription)),
                                    };
                                    if tx_dialog_in_clone.send(dialog_req).await.is_err() { break; }
                                    
                                    let final_sig = StreamConversationRequest {
                                        payload: Some(stream_conversation_request::Payload::IsFinalInput(true)),
                                    };
                                    if tx_dialog_in_clone.send(final_sig).await.is_err() { break; }
                                }
                            }
                        },
                        Err(e) => {
                            let _ = tx_ws_out_stt.send(create_telemetry("stt", "error", &e.to_string())).await;
                            error!("STT Stream Error: {}", e);
                        },
                    }
                }
            },
            Err(e) => error!("Failed to start STT stream: {}", e),
        }
    });

    // --- 2. DIALOG LOOP ---
    let mut dialog_client = clients.dialog.clone();
    let tts_client = clients.tts.clone();
    let tx_ws_out_dialog = tx_ws_out.clone();
    let session_id_dialog = session_id.clone();

    let init_req = StreamConversationRequest {
        payload: Some(stream_conversation_request::Payload::Config(ConversationConfig {
            session_id: session_id.clone(),
            user_id: "simulation_bot".to_string(),
        })),
    };
    let _ = tx_dialog_in.send(init_req).await;

    let dialog_handle = tokio::spawn(async move {
        let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx_dialog_in);
        
        let mut request = tonic::Request::new(request_stream);
        if let Ok(meta_val) = MetadataValue::from_str(&session_id_dialog) {
            request.metadata_mut().insert("x-trace-id", meta_val);
        }

        // [GÜNCELLEME] Cümle Tamponu
        let mut sentence_buffer = SentenceBuffer::new();

        match dialog_client.stream_conversation(request).await {
            Ok(response) => {
                let mut inbound = response.into_inner();
                while let Some(result) = inbound.next().await {
                    match result {
                        Ok(dialog_resp) => {
                            match dialog_resp.payload {
                                Some(stream_conversation_response::Payload::TextResponse(text)) => {
                                    // 1. Text'i ham olarak WebSocket'e gönder (Altyazı için anlık akış gerekir)
                                    let json = serde_json::json!({ "type": "subtitle", "text": text });
                                    let _ = tx_ws_out_dialog.send(Message::Text(json.to_string())).await;

                                    // 2. Text'i TTS için tamponla
                                    if let Some(complete_sentence) = sentence_buffer.push_and_extract(&text) {
                                        info!("TTS Generating Sentence: '{}'", complete_sentence);
                                        let _ = tx_ws_out_dialog.send(create_telemetry("llm", "sentence_complete", &complete_sentence)).await;
                                        
                                        // TTS İsteği (Asenkron)
                                        let tts_req = SynthesizeStreamRequest {
                                            text: complete_sentence,
                                            text_type: TextType::Text as i32, 
                                            voice_id: voice_id.clone(), 
                                            audio_config: None,
                                            preferred_provider: "auto".to_string(),
                                            prosody: None,
                                        };
                                        
                                        let mut t_client = tts_client.clone();
                                        let tx_out = tx_ws_out_dialog.clone();
                                        let sid = session_id_dialog.clone();
                                        
                                        tokio::spawn(async move {
                                            let mut req = tonic::Request::new(tts_req);
                                            if let Ok(mv) = MetadataValue::from_str(&sid) {
                                                req.metadata_mut().insert("x-trace-id", mv);
                                            }

                                            match t_client.synthesize_stream(req).await {
                                                Ok(tts_res) => {
                                                    let mut tts_stream = tts_res.into_inner();
                                                    while let Some(audio_res) = tts_stream.next().await {
                                                        if let Ok(chunk) = audio_res {
                                                            let _ = tx_out.send(Message::Binary(chunk.audio_content)).await;
                                                        }
                                                    }
                                                },
                                                Err(e) => {
                                                     warn!("TTS Error: {}", e);
                                                }
                                            }
                                        });
                                    }
                                },
                                // LLM Cevabı bittiğinde, tamponda kalan son parçayı da gönder
                                Some(stream_conversation_response::Payload::IsFinalResponse(_)) => {
                                     if let Some(remaining) = sentence_buffer.flush() {
                                        info!("TTS Flushing Remaining: '{}'", remaining);
                                        // Kod tekrarı olmaması için burayı fonksiyonlaştırabiliriz ama şimdilik copy-paste
                                        let tts_req = SynthesizeStreamRequest {
                                            text: remaining,
                                            text_type: TextType::Text as i32, 
                                            voice_id: voice_id.clone(), 
                                            audio_config: None,
                                            preferred_provider: "auto".to_string(),
                                            prosody: None,
                                        };
                                        let mut t_client = tts_client.clone();
                                        let tx_out = tx_ws_out_dialog.clone();
                                        let sid = session_id_dialog.clone();
                                        tokio::spawn(async move {
                                            let mut req = tonic::Request::new(tts_req);
                                            if let Ok(mv) = MetadataValue::from_str(&sid) { req.metadata_mut().insert("x-trace-id", mv); }
                                            if let Ok(tts_res) = t_client.synthesize_stream(req).await {
                                                let mut tts_stream = tts_res.into_inner();
                                                while let Some(audio_res) = tts_stream.next().await {
                                                    if let Ok(chunk) = audio_res { let _ = tx_out.send(Message::Binary(chunk.audio_content)).await; }
                                                }
                                                let _ = tx_out.send(create_telemetry("tts", "complete", "Final audio chunk")).await;
                                            }
                                        });
                                     }
                                },
                                _ => {}
                            }
                        },
                        Err(e) => error!("Dialog Stream Error: {}", e),
                    }
                }
            },
            Err(e) => error!("Failed to start Dialog stream: {}", e),
        }
    });

    // --- 3. WS WRITE LOOP ---
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = rx_ws_out.recv().await {
            if ws_sender.send(msg).await.is_err() { break; }
        }
    });

    // --- 4. WS READ LOOP ---
    while let Some(Ok(msg)) = ws_receiver.next().await {
        match msg {
            Message::Binary(data) => {
                counter!("stream_gateway_audio_bytes_received", data.len() as u64);
                let req = TranscribeStreamRequest { audio_chunk: data };
                if tx_stt_in.send(req).await.is_err() { break; }
            },
            Message::Text(text) => {
                info!("[{}] User Text: {}", session_id, text);
                let _ = tx_ws_out.send(create_telemetry("client", "text_input", &text)).await;
                let req = StreamConversationRequest {
                    payload: Some(stream_conversation_request::Payload::TextInput(text)),
                };
                if tx_dialog_in.send(req).await.is_err() { break; }
                let fin = StreamConversationRequest {
                    payload: Some(stream_conversation_request::Payload::IsFinalInput(true)),
                };
                if tx_dialog_in.send(fin).await.is_err() { break; }
            },
            Message::Close(_) => break,
            _ => {},
        }
    }

    info!("🔌 Client Disconnected. Session: {}", session_id);
    decrement_gauge!("stream_gateway_active_sessions", 1.0);
    write_handle.abort();
    stt_handle.abort();
    dialog_handle.abort();
}