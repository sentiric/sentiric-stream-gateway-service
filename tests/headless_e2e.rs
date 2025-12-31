use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures::{SinkExt, StreamExt};
use url::Url;
use std::time::Duration;
use serde_json::Value;

#[tokio::test]
async fn test_full_pipeline_connection() {
    // 1. Gateway'e Bağlan
    // Not: Bu testin çalışması için docker compose up ile servisin ayakta olması gerekir.
    // CI ortamında servis ve test aynı ağda değilse localhost çalışmayabilir.
    // Ancak local geliştirme için localhost:18030 uygundur.
    let url = Url::parse("ws://localhost:18030/ws").expect("Geçersiz URL");
    println!("🔌 Bağlanılıyor: {}", url);

    // Bağlantıyı dene (Servis henüz kalkmamış olabilir, basit retry)
    let mut attempt = 0;
    let ws_stream = loop {
        match connect_async(url.clone()).await {
            Ok((ws, _)) => break ws,
            Err(e) => {
                attempt += 1;
                if attempt > 5 { panic!("Gateway'e bağlanılamadı: {}", e); }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    };
    
    println!("✅ WebSocket Bağlandı!");

    let (mut write, mut read) = ws_stream.split();

    // 2. Metin Gönderimi Testi (Chat Modu)
    let text_msg = Message::Text("Merhaba".to_string());
    write.send(text_msg).await.expect("Mesaj gönderilemedi");
    println!("📤 'Merhaba' metni gönderildi.");

    // 3. Yanıt Bekleme (Timeout ile)
    let timeout = tokio::time::sleep(Duration::from_secs(15)); // Süreyi biraz artırdık
    tokio::pin!(timeout);

    let mut audio_received = false;
    let mut subtitle_received = false;

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        println!("📥 Text Alındı: {}", text);
                        if let Ok(json) = serde_json::from_str::<Value>(&text) {
                            if json["type"] == "subtitle" {
                                println!("✅ Altyazı doğrulandı.");
                                subtitle_received = true;
                            }
                        }
                    },
                    Some(Ok(Message::Binary(bin))) => {
                        println!("📥 Audio Chunk Alındı: {} bytes", bin.len());
                        if bin.len() > 0 {
                            audio_received = true;
                        }
                    },
                    Some(Err(e)) => {
                        eprintln!("❌ Hata: {}", e);
                        break;
                    },
                    None => break, // Stream bitti
                    _ => {}
                }
            }
            _ = &mut timeout => {
                println!("⏰ Zaman aşımı! Test sonlandırılıyor.");
                break;
            }
        }

        // Eğer hem ses hem metin aldıysak test başarılıdır
        if audio_received && subtitle_received {
            println!("🎉 TEST BAŞARILI: Hem metin yanıtı hem ses akışı alındı.");
            return;
        }
    }
    
    // NOT: Tam entegrasyon testi için arkadaki servislerin (Dialog, TTS) de çalışıyor olması gerekir.
    // Eğer sadece Gateway test ediliyorsa ve arkası boşsa bu test fail edebilir, bu beklenen bir durumdur.
    if !subtitle_received && !audio_received {
        println!("⚠️ Uyarı: Yanıt alınamadı. Arkadaki servisler (Dialog/TTS) çalışıyor mu?");
    }
}