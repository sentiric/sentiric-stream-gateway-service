use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, error};
use uuid::Uuid;
use crate::clients::GrpcClients;

// Contracts
use sentiric_contracts::sentiric::{
    stt::v1::{TranscribeStreamRequest},
    dialog::v1::{
        StreamConversationRequest, 
        stream_conversation_request, stream_conversation_response,
        ConversationConfig
    },
    tts::v1::{SynthesizeStreamRequest, TextType},
    // NOT: stream::v1 importları şimdilik kaldırıldı (unused warning için).
    // İleride Protobuf implementation eklenince geri açılacak.
};

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(clients): State<Arc<GrpcClients>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, clients))
}

async fn handle_socket(socket: WebSocket, clients: Arc<GrpcClients>) {
    let session_id = Uuid::new_v4().to_string();
    info!("📱 New Stream Connection. Session: {}", session_id);

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // --- CHANNELS FOR PIPELINE ---
    // WS -> STT
    let (tx_stt_in, rx_stt_in) = mpsc::channel::<TranscribeStreamRequest>(100);
    // WS (Text) -> Dialog
    let (tx_dialog_in, rx_dialog_in) = mpsc::channel::<StreamConversationRequest>(100);
    
    // --- 1. STT LOOP (Audio -> Text) ---
    let mut stt_client = clients.stt.clone();
    let tx_dialog_in_clone = tx_dialog_in.clone();
    let session_id_clone = session_id.clone();

    tokio::spawn(async move {
        let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx_stt_in);
        
        match stt_client.transcribe_stream(request_stream).await {
            Ok(response) => {
                let mut inbound = response.into_inner();
                while let Some(result) = inbound.next().await {
                    match result {
                        Ok(stt_resp) => {
                            if !stt_resp.partial_transcription.is_empty() && stt_resp.is_final {
                                info!("[{}] STT Final: {}", session_id_clone, stt_resp.partial_transcription);
                                
                                // Text'i Dialog'a gönder
                                let dialog_req = StreamConversationRequest {
                                    payload: Some(stream_conversation_request::Payload::TextInput(stt_resp.partial_transcription)),
                                };
                                if let Err(_) = tx_dialog_in_clone.send(dialog_req).await { break; }
                                
                                // Final sinyalini gönder
                                let final_sig = StreamConversationRequest {
                                    payload: Some(stream_conversation_request::Payload::IsFinalInput(true)),
                                };
                                if let Err(_) = tx_dialog_in_clone.send(final_sig).await { break; }
                            }
                        },
                        Err(e) => error!("STT Stream Error: {}", e),
                    }
                }
            },
            Err(e) => error!("Failed to start STT stream: {}", e),
        }
    });

    // --- 2. DIALOG LOOP (Text -> Response) ---
    let mut dialog_client = clients.dialog.clone();
    let tts_client = clients.tts.clone();
    
    // WS Writer Channel
    let (tx_ws_out, mut rx_ws_out) = mpsc::channel::<Message>(100);

    // Initial Dialog Handshake
    let init_req = StreamConversationRequest {
        payload: Some(stream_conversation_request::Payload::Config(ConversationConfig {
            session_id: session_id.clone(),
            user_id: "anonymous".to_string(),
        })),
    };
    let _ = tx_dialog_in.send(init_req).await;

    tokio::spawn(async move {
        let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx_dialog_in);
        
        match dialog_client.stream_conversation(request_stream).await {
            Ok(response) => {
                let mut inbound = response.into_inner();
                while let Some(result) = inbound.next().await {
                    match result {
                        Ok(dialog_resp) => {
                            match dialog_resp.payload {
                                Some(stream_conversation_response::Payload::TextResponse(text)) => {
                                    // 1. İstemciye Alt Yazı Olarak Gönder
                                    let json = serde_json::json!({ "type": "subtitle", "text": text });
                                    let _ = tx_ws_out.send(Message::Text(json.to_string())).await;

                                    // 2. TTS'e Gönder (Seslendirme)
                                    let tts_req = SynthesizeStreamRequest {
                                        text: text.clone(),
                                        text_type: TextType::Text as i32, 
                                        voice_id: "mms:tur".to_string(),
                                        audio_config: None,
                                        preferred_provider: "mms".to_string(),
                                        prosody: None,
                                    };
                                    
                                    // TTS Call
                                    let mut t_client = tts_client.clone();
                                    let tx_out = tx_ws_out.clone();
                                    
                                    tokio::spawn(async move {
                                        let req = tonic::Request::new(tts_req);
                                        if let Ok(tts_res) = t_client.synthesize_stream(req).await {
                                            let mut tts_stream = tts_res.into_inner();
                                            while let Some(audio_res) = tts_stream.next().await {
                                                if let Ok(chunk) = audio_res {
                                                    let _ = tx_out.send(Message::Binary(chunk.audio_content)).await;
                                                }
                                            }
                                        }
                                    });
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
    // DÜZELTME: `mut` kaldırıldı
    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx_ws_out.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // --- 4. WS READ LOOP (Main Control) ---
    while let Some(Ok(msg)) = ws_receiver.next().await {
        match msg {
            Message::Binary(data) => {
                // Fallback: Raw Audio (Web UI)
                let req = TranscribeStreamRequest { audio_chunk: data };
                if tx_stt_in.send(req).await.is_err() { break; }
            },
            Message::Text(text) => {
                // Raw Text (Chat)
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
    write_task.abort();
}

// FUTURE IMPLEMENTATION: Protobuf Handler Planı
/*
async fn handle_protobuf_request(
    req: StreamSessionRequest,
    tx_stt: &mpsc::Sender<TranscribeStreamRequest>,
    tx_dialog: &mpsc::Sender<StreamConversationRequest>
) {
    match req.data {
        Some(stream_session_request::Data::AudioChunk(chunk)) => {
            let _ = tx_stt.send(TranscribeStreamRequest { audio_chunk: chunk }).await;
        },
        Some(stream_session_request::Data::TextMessage(text)) => {
            // ...
        },
        Some(stream_session_request::Data::Config(cfg)) => {
            // Reconfigure session settings (language, voice etc.)
        },
        _ => {}
    }
}
*/