# 🏷️ Teknik Özellikler

## Kimlik
*   **Servis Adı:** `sentiric-stream-gateway-service`
*   **Sürüm:** `0.2.0`
*   **Dil:** Rust (Edition 2021)
*   **Framework:** Axum (Web), Tonic (gRPC), Tokio (Runtime)

## Ağ Topolojisi
*   **Ağ:** `sentiric-net` (Docker Network)
*   **IP:** `10.88.80.3` (Sabit IP)
*   **Portlar:**
    *   `18030` (HTTP/WebSocket) - Dışa Açık
    *   `18032` (Metrics) - İç Ağ

## Bağımlılıklar
Bu servis aşağıdaki upstream servisleri tüketir:
1.  **STT Gateway:** `15021` (gRPC / mTLS)
2.  **Dialog Service:** `12061` (gRPC / mTLS)
3.  **TTS Gateway:** `14011` (gRPC / mTLS)

## Kaynak Kullanımı (Tahmini)
*   **CPU:** Boşta <%1, Yükte (100 stream) ~%20 (Single Core)
*   **RAM:** ~20MB (Başlangıç), ~100MB (Yükte)