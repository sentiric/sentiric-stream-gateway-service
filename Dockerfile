FROM rust:1.84-bookworm as builder
WORKDIR /app

# Protobuf derleyicisi gerekli (sentiric-contracts için)
RUN apt-get update && apt-get install -y protobuf-compiler cmake

COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
# Runtime için gerekli sertifika paketleri ve SSL
RUN apt-get update && apt-get install -y ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*

# Binary ismi değişti: sentiric-stream-gateway-service
COPY --from=builder /app/target/release/sentiric-stream-gateway-service /usr/local/bin/
COPY static /app/static 

WORKDIR /app
ENV RUST_LOG=info
CMD ["sentiric-stream-gateway-service"]