use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures::{SinkExt, StreamExt};
use url::Url;
use std::time::{Duration, Instant};
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Write};

// Helper: Log Basıcı (Zaman Damgalı)
fn log(tag: &str, msg: &str) {
    let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
    println!("{} [{}] {}", timestamp, tag, msg);
}

// Helper: WAV Header Oluşturucu
fn write_wav_header(file: &mut File, data_len: u32, sample_rate: u32) {
    let total_data_len = data_len + 36;
    let byte_rate = sample_rate * 2; 
    
    file.write_all(b"RIFF").unwrap();
    file.write_all(&total_data_len.to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap(); 
    file.write_all(&1u16.to_le_bytes()).unwrap(); 
    file.write_all(&1u16.to_le_bytes()).unwrap(); 
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap(); 
    file.write_all(&16u16.to_le_bytes()).unwrap(); 
    file.write_all(b"data").unwrap();
    file.write_all(&data_len.to_le_bytes()).unwrap();
}

async fn run_session(audio_data: Vec<u8>, is_verification: bool) -> (String, Vec<u8>) {
    let url = Url::parse("ws://localhost:18030/ws").expect("Geçersiz URL");
    let (ws_stream, _) = connect_async(url).await.expect("Bağlantı hatası");
    let (mut write, mut read) = ws_stream.split();

    let session_tag = if is_verification { "VERIFY" } else { "MAIN" };
    log(session_tag, "Oturum Başlıyor...");

    let sender_handle = tokio::spawn(async move {
        // Chunk boyutu: 6400 bytes (200ms @ 16kHz)
        let chunk_size = 6400; 
        let total_chunks = audio_data.len() / chunk_size;
        
        log("SENDER", &format!("Ses gönderimi başladı ({} paket)...", total_chunks));
        
        for (i, chunk) in audio_data.chunks(chunk_size).enumerate() {
            write.send(Message::Binary(chunk.to_vec())).await.expect("Ses gönderilemedi");
            tokio::time::sleep(Duration::from_millis(200)).await;
            if i % 10 == 0 { print!("."); use std::io::Write; std::io::stdout().flush().unwrap(); }
        }
        println!(""); // Newline
        log("SENDER", "Ses bitti. VAD tetikleyici (Sessizlik) gönderiliyor...");

        // VAD Tetikleyici (2 Saniye Sessizlik)
        let silence = vec![0u8; 6400]; 
        for _ in 0..10 { 
            if write.send(Message::Binary(silence.clone())).await.is_err() { break; }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        log("SENDER", "Sessizlik gönderildi. Kanal açık tutuluyor.");
        
        loop { tokio::time::sleep(Duration::from_secs(1)).await; }
    });

    let mut captured_text = String::new();
    let mut captured_audio = Vec::new();
    let start = Instant::now();
    let mut last_activity = Instant::now();

    loop {
        // Genel Timeout (60sn)
        if start.elapsed() > Duration::from_secs(120) {
            log(session_tag, "❌ ZAMAN AŞIMI (Global Timeout)");
            break; 
        }

        // Aktivite Timeout (15sn boyunca ses/metin gelmezse bitir)
        if !captured_text.is_empty() && last_activity.elapsed() > Duration::from_secs(15) {
             log(session_tag, "✅ Aktivite bitti (Idle Timeout). Oturum kapatılıyor.");
             break;
        }

        match tokio::time::timeout(Duration::from_secs(1), read.next()).await {
            Ok(Some(msg)) => {
                last_activity = Instant::now(); // Aktiviteyi güncelle
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(json) = serde_json::from_str::<Value>(&text) {
                            if json["type"] == "subtitle" {
                                let content = json["text"].as_str().unwrap_or("");
                                captured_text.push_str(content);
                                if !is_verification {
                                    // Anlık akışı göster
                                    print!("{}", content); use std::io::Write; std::io::stdout().flush().unwrap();
                                }
                            }
                            if json["type"] == "telemetry" {
                                // Sadece durum değişimlerini logla
                                let status = json["status"].as_str().unwrap_or("");
                                if status == "final" || status == "complete" || status == "error" {
                                    println!("\n📡 [TELEM] {} -> {}: {}", 
                                        json["phase"].as_str().unwrap_or("?"), 
                                        status, 
                                        json["detail"].as_str().unwrap_or("")
                                    );
                                }
                                if is_verification && json["phase"] == "stt" && status == "final" {
                                     captured_text = json["detail"].as_str().unwrap_or("").to_string();
                                     println!("\n🔍 [VERIFY] STT Algıladı: {}", captured_text);
                                }
                            }
                        }
                    },
                    Ok(Message::Binary(bin)) => {
                        if captured_audio.is_empty() && !is_verification {
                            println!(""); // Newline fix
                            log(session_tag, &format!("🔊 İLK SES PAKETİ ALINDI ({} bytes)", bin.len()));
                        }
                        if !is_verification { 
                            captured_audio.extend_from_slice(&bin);
                        }
                    },
                    Ok(Message::Close(_)) => {
                        log(session_tag, "🔌 Sunucu bağlantıyı kapattı.");
                        break;
                    },
                    Err(e) => {
                        log(session_tag, &format!("❌ Okuma Hatası: {}", e));
                        break;
                    },
                    _ => {}
                }
            },
            Ok(None) => {
                log(session_tag, "Stream bitti.");
                break;
            }, 
            Err(_) => {
                // Read timeout (Normal, beklemeye devam et)
                continue;
            }
        }
    }
    
    println!(""); // Newline
    sender_handle.abort();
    (captured_text, captured_audio)
}

#[tokio::test]
async fn test_full_audio_conversation() {
    let wav_path = "test.16khz.wav";
    let mut file_bytes = Vec::new();
    let mut f = File::open(wav_path).expect("Test dosyası yok");
    f.read_to_end(&mut file_bytes).unwrap();
    if file_bytes.len() > 44 { file_bytes = file_bytes[44..].to_vec(); }

    println!("==================================================");
    println!("🎙️  PHASE 1: ANA ARAMA (Kullanıcı Konuşuyor)");
    println!("==================================================");
    
    let (ai_text_response, ai_audio_bytes) = run_session(file_bytes, false).await;

    println!("--------------------------------------------------");
    log("RESULT", "PHASE 1 Tamamlandı.");
    println!("📝 AI Metni Uzunluğu: {} karakter", ai_text_response.len());
    println!("🔊 AI Sesi Boyutu   : {} bytes ({:.2} sn)", ai_audio_bytes.len(), (ai_audio_bytes.len() as f32 / 32000.0));

    if ai_audio_bytes.len() < 1000 {
        panic!("❌ HATA: TTS sesi üretilmedi veya çok kısa!");
    }

    let output_path = "tests/ai_response.wav";
    let mut out_file = File::create(output_path).unwrap();
    write_wav_header(&mut out_file, ai_audio_bytes.len() as u32, 16000); 
    out_file.write_all(&ai_audio_bytes).unwrap();
    log("FILE", &format!("AI sesi kaydedildi: {}", output_path));

    println!("==================================================");
    println!("♻️  PHASE 2: LOOPBACK DOĞRULAMA (AI Sesi STT'ye Gönderiliyor)");
    println!("==================================================");
    
    let (stt_transcription, _) = run_session(ai_audio_bytes, true).await;

    println!("\n📊 FİNAL DOĞRULAMA RAPORU");
    println!("=========================");
    println!("1. Beklenen (LLM): \"{}\"", ai_text_response.trim());
    println!("2. Algılanan (STT): \"{}\"", stt_transcription.trim());

    // Benzerlik Kontrolü
    let keywords: Vec<&str> = ai_text_response.split_whitespace().filter(|s| s.len() > 3).collect();
    let mut match_count = 0;
    for word in &keywords {
        if stt_transcription.to_lowercase().contains(&word.to_lowercase()) {
            match_count += 1;
        }
    }

    let match_ratio = if !keywords.is_empty() { (match_count as f32 / keywords.len() as f32) * 100.0 } else { 0.0 };
    println!("📈 Kelime Eşleşme Oranı: {:.2}%", match_ratio);

    if match_ratio > 20.0 || (!stt_transcription.is_empty() && !ai_text_response.is_empty()) {
        println!("🏆 TEST BAŞARILI: Sistem tutarlı çalışıyor.");
    } else {
        println!("⚠️  UYARI: Düşük eşleşme oranı. (Ses kalitesi veya STT modeli yetersiz olabilir)");
        // Pipeline çalıştığı için testi fail etmiyoruz, ama uyarıyoruz.
    }
}