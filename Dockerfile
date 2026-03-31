# --- Builder Stage ---
# Rust 1.85 Edition 2024 desteği için zorunludur
FROM rust:1.85-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Dependency Caching
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src

# Final Build
COPY . .
RUN cargo build --release

# --- Final Stage ---
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

RUN useradd -u 1001 -ms /bin/bash sentiric
USER sentiric
WORKDIR /home/sentiric

COPY --from=builder /app/target/release/sentiric-stream-gateway-service .

ENV RUST_LOG=info
ENV LOG_FORMAT=json
EXPOSE 18030

ENTRYPOINT ["./sentiric-stream-gateway-service"]