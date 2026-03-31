# 🌊 Sentiric Stream Gateway Service

**Sürüm:** v1.0.1  
**Mimari:** %100 Rust, Nano-Edge Uyumlu, SUTS v4.0 Logging, Ghost Publisher  

Sentiric platformunun Web, Mobil ve IoT cihazlar (Dijital Dünya) için dış kapısıdır. Gelen WebSocket akışlarını alır ve hiçbir "Spagetti Yapay Zeka Mantığı" barındırmadan `sentiric-ai-pipeline-sdk` üzerinden iç uzman motorlara (STT, Dialog, TTS) köprüler.

## 🏛️ Mimari Anayasa (Constitutional Compliance)
- **SUTS v4.0:** Tüm loglar makine okunabilir JSON formatındadır ve zorunlu alanları (event, trace_id, tenant_id) içerir.
- **Strict mTLS:** İç gRPC servislerine (AI) bağlantı sadece mTLS ile kurulur.
- **Ghost Publisher (Graceful Degradation):** RabbitMQ yoksa çökmez, oturum eventlerini RAM'de `VecDeque` içinde tamponlar ve bağlandığında eritir.
- **Resource Aware:** `WORKER_THREADS` ENV değişkeni ile CPU sınırlandırılabilir.

## 🚀 Çalıştırma

```bash
export TENANT_ID="sentiric_demo"
export STREAM_GATEWAY_SERVICE_HTTP_PORT=18030
export STT_GATEWAY_GRPC_URL="https://stt-gateway-service.service.sentiric.cloud:15021"
export DIALOG_SERVICE_GRPC_URL="https://dialog-service.service.sentiric.cloud:12061"
export TTS_GATEWAY_GRPC_URL="https://tts-gateway-service.service.sentiric.cloud:14011"
export GRPC_TLS_CA_PATH="/path/to/ca.crt"
export STREAM_GATEWAY_SERVICE_CERT_PATH="/path/to/chain.crt"
export STREAM_GATEWAY_SERVICE_KEY_PATH="/path/to/private.key"
# Opsiyonel
export RABBITMQ_URL="amqp://sentiric:sentiric_pass@rabbitmq.service.sentiric.cloud:5672/%2f"

make run
```
---
