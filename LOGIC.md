# 🧠 Akış Mantığı (Internal Logic)

Bu servis, Rust'ın `Tokio` asenkron çalışma zamanı üzerinde, **Event-Driven** (Olay Güdümlü) bir mimari ile çalışır.

## 1. WebSocket Handler (`ws_handler`)

Bir istemci bağlandığında (`/ws`), `src/handlers.rs` dosyasındaki `handle_socket` fonksiyonu devreye girer ve şu işlemleri yapar:

1.  **Session ID Üretimi:** `UUID v4` formatında bir oturum kimliği oluşturulur.
2.  **Kanal Kurulumu (Channel Setup):** Servis içi iletişim için `MPSC` (Multi-Producer, Single-Consumer) kanalları açılır.
3.  **Görev Dağıtımı (Task Spawning):** 3 ana asenkron görev başlatılır:

### A. STT Loop (Audio -> Text)
*   WebSocket'ten gelen binary ses verilerini (`audio_chunk`) dinler.
*   `stt-gateway` servisine gRPC stream olarak iletir.
*   Dönen **Final** transkripsiyonu yakalar ve `Dialog Loop` kanalına atar.

### B. Dialog Loop (Text -> AI Response)
*   STT'den veya WebSocket Text mesajlarından gelen metinleri dinler.
*   `dialog-service`'in `StreamConversation` metodunu çağırır.
*   Gelen AI metin yanıtını:
    1.  JSON formatında WebSocket'e yazar (Altyazı için).
    2.  TTS servisine gönderir (Seslendirme için).

### C. WebSocket Write Loop
*   Sistemin herhangi bir yerinden (Dialog, TTS vb.) gelen mesajları istemciye iletmekten sorumludur.

## 2. Lazy Connection Stratejisi

Servis, başlangıçta (`main.rs`) diğer mikroservislere (STT, TTS, Dialog) **bağlanmaz**. Sadece uç noktaları (Endpoints) yapılandırır.

*   **Neden?** Mikroservis mimarisinde, bağımlı olduğunuz servis o an ayakta olmayabilir (Deployment, Restart vb.).
*   **Nasıl?** `tonic::transport::Endpoint::connect_lazy()` kullanılarak, bağlantı girişimi **ilk gerçek istek** gelene kadar ertelenir.
*   **Sonuç:** `stream-gateway`, arka plan servisleri kapalı olsa bile çökmez (Crash Loop Backoff engellenir).

## 3. Güvenlik (mTLS)

Tüm gRPC istemcileri (`src/clients.rs`), `sentiric-certificates` dizinindeki sertifikaları kullanarak oluşturulur.
*   **CA:** `ca.crt` (Sunucuyu doğrulamak için)
*   **Client Cert:** `stream-gateway-service.crt` (Kendini tanıtmak için)