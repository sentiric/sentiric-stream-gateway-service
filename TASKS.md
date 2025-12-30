# 📋 Stream Gateway - Görev Listesi

Bu belge, servisin mevcut durumunu ve gelecekte yapılması planlanan geliştirmeleri içerir.

## ✅ Tamamlananlar (MVP v0.2.0)
- [x] **Yeniden Markalama:** `mobile-gateway` -> `stream-gateway` dönüşümü.
- [x] **Altyapı:** Dockerfile, Compose, Makefile ve CI/CD pipeline'ı.
- [x] **Resilience:** Lazy Connection mimarisi ile servis bağımsızlığı.
- [x] **Güvenlik:** mTLS entegrasyonu (Client Side).
- [x] **Pipeline:** STT -> Dialog -> TTS akışının Rust kanalları ile yönetimi.
- [x] **UI:** Modern, WebSocket Secure (WSS) destekli test arayüzü.

## 🚀 Gelecek Planları (Backlog)

### Protobuf over WebSocket
*   **Durum:** Şu an JSON ve Raw Binary kullanılıyor.
*   **Hedef:** `sentiric.stream.v1.StreamSessionRequest` mesajlarını WebSocket üzerinden binary frame içinde taşımak.
*   **Fayda:** Daha düşük bant genişliği ve tip güvenliği.

### Metrics Exporter
*   **Durum:** Loglama var, metrik yok.
*   **Hedef:** Prometheus için `/metrics` endpoint'i (Aktif bağlantı sayısı, ses işleme süresi vb.).