# Build Stage
FROM rust:1.93-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src
COPY . .
RUN cargo build --release

# Final Stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Run as non-root user (Security Best Practice)
RUN useradd -ms /bin/bash sentiric
USER sentiric
WORKDIR /home/sentiric

COPY --from=builder /app/target/release/sentiric-stream-gateway-service .

ENV RUST_LOG=info
EXPOSE 18030

ENTRYPOINT ["./sentiric-stream-gateway-service"]