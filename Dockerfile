# --- Builder Stage ---
FROM rust:1.84-slim-bookworm AS builder

# Gerekli sistem bağımlılıkları (Derleme ve Git bağımlılıkları için)
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Bağımlılıkları önbelleğe almak için boş proje oluştur
COPY Cargo.toml ./
# Cargo.lock varsa ekle, yoksa hata almamak için dummy oluşturulabilir
# Git bağımlılıkları (sdk, contracts) fetch edilebilmesi için Cargo.toml kopyalanmalı
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src

# Kaynak kodları kopyala ve derle
COPY . .
RUN cargo build --release

# --- Final Stage ---
FROM debian:bookworm-slim

# Runtime kütüphaneleri
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# [ARCH-COMPLIANCE] Güvenlik: Root olmayan kullanıcı (UID 1001)
RUN useradd -u 1001 -ms /bin/bash sentiric
USER sentiric
WORKDIR /home/sentiric

# Binary'yi kopyala
COPY --from=builder /app/target/release/sentiric-stream-gateway-service .

# SUTS v4.0 Standardı için default envs
ENV RUST_LOG=info
ENV LOG_FORMAT=json

EXPOSE 18030

ENTRYPOINT ["./sentiric-stream-gateway-service"]