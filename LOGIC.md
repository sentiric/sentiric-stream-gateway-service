# 🧬 Stream Gateway Logic
1. **Full-Duplex Pipeline:** `sentiric-ai-pipeline-sdk` kullanarak STT-LLM-TTS döngüsünü tarayıcıya bağlar.
2. **Handover Relay:** Bir oturum "Ajan Moduna" geçtiğinde, AI Pipeline durdurulur ve paketler (audio_chunk) Ajan WebSocket'ine aynalanır (Mirroring).
