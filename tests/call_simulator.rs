use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures::{SinkExt, StreamExt};
use url::Url;
use std::time::{Duration, Instant};
use serde_json::Value;
use std::fs::File;
use std::io::Read;

#[tokio::test]
async fn test_full_audio_conversation() {
    let url = Url::parse("ws://localhost:18030/ws").expect("Geçersiz URL");
    println!("📞 [SIM] Aramak yapılıyor: {}", url);

    // 1. WebSocket Bağlantısı
    let (ws_stream, _) = connect_async(url).await.expect("Bağlantı hatası: Sunucu ayakta mı?");
    println!("✅ [SIM] Bağlantı Kuruldu.");
    let (mut write, mut read) = ws_stream.split();

    // 2. Ses Dosyasını Oku
    let wav_path = "test.16khz.wav";
    let mut file_bytes = Vec::new();
    
    if let Ok(mut f) = File::open(wav_path) {
        f.read_to_end(&mut file_bytes).expect("Dosya okunamadı");
        // WAV header'ı atla (44 byte)
        if file_bytes.len() > 44 { file_bytes = file_bytes[44..].to_vec(); }
        println!("🎤 [SIM] Ses dosyası yüklendi ({} bytes)", file_bytes.len());
    } else {
        panic!("❌ [SIM] '{}' bulunamadı! Lütfen geçerli bir test dosyası koyun.", wav_path);
    }

    // 3. Akış Başlat (Streaming)
    let chunk_size = 6400; // 200ms
    let chunks: Vec<Vec<u8>> = file_bytes.chunks(chunk_size).map(|s| s.to_vec()).collect();
    
    // [FIX] Kullanılmayan değişken uyarısını önlemek için kullanıyoruz
    let total_chunks = chunks.len();

    let sender_handle = tokio::spawn(async move {
        // A. Gerçek Sesi Gönder
        for (i, chunk) in chunks.into_iter().enumerate() {
            write.send(Message::Binary(chunk)).await.expect("Ses gönderilemedi");
            tokio::time::sleep(Duration::from_millis(200)).await;
            if i % 10 == 0 { print!("."); } 
        }
        println!("\n✅ [SIM] Ses bitti ({}/{}). VAD tetikleyici gönderiliyor...", total_chunks, total_chunks);

        // B. VAD Tetikleyici Sessizlik (2 Saniye)
        let silence_chunk = vec![0u8; 6400];
        for _ in 0..10 { 
            // Hata alırsak döngüyü kır (Bağlantı kapanmış olabilir)
            if write.send(Message::Binary(silence_chunk.clone())).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        println!("✅ [SIM] Sessizlik gönderildi. Yanıt bekleniyor...");

        // Keep-alive
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // 4. Yanıtları Dinle
    let mut received_subtitle = false;
    let mut received_audio = false;
    let mut stt_final_received = false;
    
    let start_listen = Instant::now();
    let max_wait = Duration::from_secs(60);

    loop {
        if start_listen.elapsed() > max_wait {
            println!("\n⏰ [SIM] ZAMAN AŞIMI! Yanıt gelmedi.");
            break;
        }

        match tokio::time::timeout(Duration::from_secs(1), read.next()).await {
            Ok(Some(msg)) => {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(json) = serde_json::from_str::<Value>(&text) {
                            let msg_type = json["type"].as_str().unwrap_or("");
                            
                            if msg_type == "subtitle" {
                                println!("\n📝 [SIM] ALTYAZI (LLM): {}", json["text"]);
                                received_subtitle = true;
                            } else if msg_type == "telemetry" {
                                let phase = json["phase"].as_str().unwrap_or("?");
                                let status = json["status"].as_str().unwrap_or("?");
                                let detail = json["detail"].as_str().unwrap_or("");
                                
                                if status == "final" || status == "complete" || status == "error" {
                                    println!("📡 [TELEM] {} -> {}: {}", phase, status, detail);
                                }

                                if phase == "stt" && status == "final" {
                                    stt_final_received = true;
                                    println!("✨ [SIM] STT TESPİT EDİLDİ: '{}'", detail);
                                }
                            }
                        }
                    },
                    Ok(Message::Binary(bin)) => {
                        if !received_audio {
                            println!("\n🔊 [SIM] SES YANITI BAŞLADI (TTS) - İlk paket: {} bytes", bin.len());
                            received_audio = true;
                        }
                    },
                    Ok(Message::Close(_)) => {
                        println!("\n🔌 [SIM] Sunucu bağlantıyı kapattı.");
                        break;
                    },
                    Err(e) => {
                        println!("\n❌ [SIM] Okuma Hatası: {}", e);
                        break;
                    },
                    _ => {}
                }
            },
            Ok(None) => break, 
            Err(_) => continue,
        }

        if stt_final_received && received_audio && received_subtitle {
            println!("\n🎉 [SIM] MÜKEMMEL! Tam Tur Başarılı.");
            break; 
        }
    }

    sender_handle.abort();

    println!("\n📊 [SIM] SONUÇ RAPORU");
    println!("--------------------------------");
    println!("STT Final Metni    : {}", if stt_final_received { "✅ EVET" } else { "❌ HAYIR" });
    println!("LLM Yanıtı (Text)  : {}", if received_subtitle { "✅ EVET" } else { "❌ HAYIR" });
    println!("TTS Sesi (Binary)  : {}", if received_audio { "✅ EVET" } else { "❌ HAYIR" });

    if stt_final_received && received_audio {
        println!("🏆 TEST BAŞARILI");
    } else {
        panic!("⚠️ TEST BAŞARISIZ");
    }
}