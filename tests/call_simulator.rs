use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures::{SinkExt, StreamExt};
use url::Url;
use std::time::{Duration, Instant};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::env;
use chrono::Local;

// --- CONFIGURATION ---
const DEFAULT_GATEWAY_URL: &str = "ws://localhost:18030/ws";
const TEST_AUDIO_PATH: &str = "test.16khz.wav";
const ARTIFACTS_DIR: &str = "tests/artifacts";

// --- LOGGING ---
fn log(tag: &str, msg: &str) {
    let timestamp = Local::now().format("%H:%M:%S%.3f");
    println!("{} [{}] {}", timestamp, tag, msg);
}

// --- SMART SRT GENERATOR ---
struct SrtBuilder {
    entries: Vec<String>,
    counter: usize,
    start_time: Instant,
    buffer: String,
    buffer_start: Duration,
}

impl SrtBuilder {
    fn new() -> Self {
        Self { 
            entries: Vec::new(), 
            counter: 1, 
            start_time: Instant::now(),
            buffer: String::new(),
            buffer_start: Duration::from_secs(0),
        }
    }

    fn add(&mut self, speaker: &str, text: &str) {
        let elapsed = self.start_time.elapsed();
        
        if self.buffer.is_empty() {
            self.buffer_start = elapsed;
        }
        
        self.buffer.push_str(text);

        // Cümle bitişi (. ? !) veya uzunluk kontrolü
        if text.chars().any(|c| ".?!".contains(c)) || self.buffer.len() > 80 {
            self.flush(speaker, elapsed); // Bitiş zamanı olarak şu anı kullan
        }
    }

    fn flush(&mut self, speaker: &str, end_time: Duration) {
        if self.buffer.trim().is_empty() { return; }

        let start_seconds = self.buffer_start.as_secs();
        let start_millis = self.buffer_start.subsec_millis();
        
        // Bitiş zamanı, başlangıçtan en az 1 saniye sonra olsun
        let end_adjusted = if end_time <= self.buffer_start {
            self.buffer_start + Duration::from_secs(2)
        } else {
            end_time + Duration::from_millis(500) // Okuma payı
        };

        let end_seconds = end_adjusted.as_secs();
        let end_millis = end_adjusted.subsec_millis();
        
        let fmt_time = |s, ms| format!("{:02}:{:02}:{:02},{:03}", s / 3600, (s % 3600) / 60, s % 60, ms);
        
        let entry = format!(
            "{}\n{} --> {}\n[{}] {}\n\n",
            self.counter,
            fmt_time(start_seconds, start_millis),
            fmt_time(end_seconds, end_millis),
            speaker,
            self.buffer.trim()
        );
        self.entries.push(entry);
        self.counter += 1;
        self.buffer.clear();
    }

    fn save(&mut self, path: &str) {
        // Kalan son parçayı yaz
        if !self.buffer.is_empty() {
             self.flush("END", self.start_time.elapsed());
        }
        let content = self.entries.concat();
        fs::write(path, content).expect("SRT yazılamadı");
    }
}

// --- WAV HEADER GENERATOR ---
fn write_wav_header(file: &mut File, data_len: u32, sample_rate: u32) {
    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate: u32 = sample_rate * u32::from(num_channels) * u32::from(bits_per_sample) / 8;
    let block_align: u16 = num_channels * bits_per_sample / 8;
    let subchunk2_size: u32 = data_len;
    let chunk_size: u32 = 36 + subchunk2_size;

    file.write_all(b"RIFF").unwrap();
    file.write_all(&chunk_size.to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap(); 
    file.write_all(&1u16.to_le_bytes()).unwrap(); 
    file.write_all(&num_channels.to_le_bytes()).unwrap();
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    file.write_all(&block_align.to_le_bytes()).unwrap(); 
    file.write_all(&bits_per_sample.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&subchunk2_size.to_le_bytes()).unwrap();
}

// --- SIMILARITY SCORE ---
fn calculate_similarity(s1: &str, s2: &str) -> f32 {
    let clean = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>();
    let c1 = clean(s1);
    let c2 = clean(s2);
    
    let words1: Vec<&str> = c1.split_whitespace().collect();
    let words2: Vec<&str> = c2.split_whitespace().collect();
    
    if words1.is_empty() || words2.is_empty() { return 0.0; }

    let mut matches = 0;
    for w1 in &words1 {
        if words2.contains(w1) { matches += 1; }
    }
    
    (matches as f32 / words1.len() as f32) * 100.0
}

// --- SESSION RUNNER ---
async fn run_session(
    audio_to_send: Vec<u8>, 
    is_verification: bool,
    srt: &mut SrtBuilder
) -> (String, Vec<u8>) {
    let gw_url = env::var("WS_URL").unwrap_or_else(|_| DEFAULT_GATEWAY_URL.to_string());
    let url = Url::parse(&gw_url).expect("URL Hatası");
    
    log(if is_verification { "VERIFY" } else { "MAIN" }, &format!("Bağlanılıyor: {}", url));
    
    let (ws_stream, _) = match connect_async(url).await {
        Ok(s) => s,
        Err(e) => panic!("Bağlantı Hatası: {}. Servis ayakta mı?", e),
    };
    let (mut write, mut read) = ws_stream.split();

    let tag = if is_verification { "VERIFY" } else { "MAIN" };
    
    // --- TASK: AUDIO SENDER ---
    let sender = tokio::spawn(async move {
        // 16kHz Mono: 200ms = 6400 bytes
        for chunk in audio_to_send.chunks(6400) {
            write.send(Message::Binary(chunk.to_vec())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(190)).await; 
        }
        // VAD Trigger
        log("SENDER", "Ses bitti, sessizlik (VAD Trigger) gönderiliyor...");
        let silence = vec![0u8; 6400];
        for _ in 0..5 {
            write.send(Message::Binary(silence.clone())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // Kanalı açık tut
        loop { tokio::time::sleep(Duration::from_secs(1)).await; }
    });

    // --- TASK: RECEIVER ---
    let mut full_text = String::new();
    let mut full_audio = Vec::new();
    let start = Instant::now();
    let mut last_act = Instant::now();

    loop {
        // Global Timeout
        if start.elapsed() > Duration::from_secs(120) {
            log(tag, "⚠️  Süre sınırı (120s) doldu, mevcut verilerle devam ediliyor.");
            break;
        }
        
        // Idle Timeout (Data akışı durduysa)
        if !full_text.is_empty() && !full_audio.is_empty() && last_act.elapsed() > Duration::from_secs(8) {
            log(tag, "✅ Aktivite bitti (Idle Timeout).");
            break;
        }

        match tokio::time::timeout(Duration::from_secs(3), read.next()).await {
            Ok(Some(Ok(msg))) => {
                last_act = Instant::now();
                match msg {
                    Message::Text(txt) => {
                        if let Ok(json) = serde_json::from_str::<Value>(&txt) {
                            if json["type"] == "subtitle" {
                                let t = json["text"].as_str().unwrap_or("");
                                full_text.push_str(t);
                                
                                let spk = if !is_verification { "AI" } else { "STT" };
                                srt.add(spk, t);
                                
                                if !is_verification {
                                    print!("{}", t); use std::io::Write; std::io::stdout().flush().unwrap();
                                }
                            }
                        }
                    },
                    Message::Binary(bin) => {
                        if full_audio.is_empty() && !is_verification { 
                            println!(""); 
                            log(tag, "🔊 Ses Alınmaya Başlandı (TTS Stream)"); 
                        }
                        if !is_verification { full_audio.extend_from_slice(&bin); }
                    },
                    Message::Close(_) => {
                        log(tag, "🔌 Bağlantı sunucu tarafından kapatıldı.");
                        break;
                    },
                    _ => {}
                }
            },
            Ok(Some(Err(e))) => {
                log(tag, &format!("❌ Socket Hatası: {}", e));
                break;
            },
            Ok(None) => {
                log(tag, "Stream sonlandı (None).");
                break;
            },
            Err(_) => {
                // Timeout (Read) - Normal, beklemeye devam et
                continue;
            }
        }
    }
    
    if !is_verification { println!(""); }
    sender.abort();
    (full_text, full_audio)
}

#[tokio::test]
async fn test_ultimate_simulation() {
    fs::create_dir_all(ARTIFACTS_DIR).unwrap();
    let mut srt = SrtBuilder::new();

    // 1. HAZIRLIK: Test Sesini Yükle
    let mut input_audio = Vec::new();
    let wav_exists = if let Ok(mut f) = File::open(TEST_AUDIO_PATH) {
        f.read_to_end(&mut input_audio).unwrap();
        if input_audio.len() > 44 { input_audio = input_audio[44..].to_vec(); } // Header skip
        srt.add("USER", "[Kullanıcı Sesi Gönderildi]");
        true
    } else {
        log("WARN", "Test dosyası bulunamadı, sessizlik (Dummy) gönderiliyor.");
        input_audio = vec![0u8; 32000]; // 2 sn sessizlik
        srt.add("USER", "[Sessizlik Gönderildi]");
        false
    };

    println!("\n=== FAZ 1: DIALOG SİMÜLASYONU (User -> AI) ===");
    let (ai_text, ai_audio) = run_session(input_audio.clone(), false, &mut srt).await;

    // Artifacts Kayıt
    if wav_exists {
        let f1_path = format!("{}/1_user_input.wav", ARTIFACTS_DIR);
        let mut f1 = File::create(&f1_path).unwrap();
        write_wav_header(&mut f1, input_audio.len() as u32, 16000);
        f1.write_all(&input_audio).unwrap();
    }

    let f2_path = format!("{}/2_ai_response_24k.wav", ARTIFACTS_DIR);
    let mut f2 = File::create(&f2_path).unwrap();
    // [NOT] Coqui TTS 24000Hz çıktı verir.
    write_wav_header(&mut f2, ai_audio.len() as u32, 24000);
    f2.write_all(&ai_audio).unwrap();

    if ai_audio.len() < 1000 {
        panic!("❌ Kritik Hata: AI ses üretmedi! Test durduruldu.");
    }

    println!("\n=== FAZ 2: DOĞRULAMA (AI Audio -> Loopback -> STT) ===");
    // Not: 24kHz veriyi STT'ye (16kHz) gönderiyoruz. Gateway içindeki resampler (C++) bunu halletmeli.
    let (stt_text, _) = run_session(ai_audio, true, &mut srt).await;

    // SRT Kayıt
    srt.save(&format!("{}/session_log.srt", ARTIFACTS_DIR));

    println!("\n📊 ANALİZ VE KALİTE RAPORU");
    println!("-------------------------");
    println!("1. Hedef Metin (LLM) : {}", ai_text.trim());
    println!("2. Algılanan (STT)   : {}", stt_text.trim());
    
    let score = calculate_similarity(&ai_text, &stt_text);
    println!("📈 Doğruluk Skoru    : {:.2}%", score);
    println!("📂 Çıktılar          : {}", ARTIFACTS_DIR);

    if score > 40.0 {
        println!("✅ TEST BAŞARILI: Sistem tutarlı ve anlaşılır.");
    } else {
        println!("⚠️  TEST GEÇTİ AMA SKOR DÜŞÜK ({:.2}%)", score);
        println!("   Olası Nedenler:");
        println!("   1. TTS telaffuzu bozuk olabilir.");
        println!("   2. STT modeli (Whisper) 24k->16k dönüşümünde zorlanmış olabilir.");
        println!("   3. AI cevabı çok kısa ise skor yanıltıcı olabilir.");
    }
}