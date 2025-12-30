# --- STAGE 1: Builder ---
FROM rust:1.84-bookworm AS builder
WORKDIR /app

# Build Args (Log amaçlı)
ARG SERVICE_VERSION
ARG GIT_COMMIT
ARG BUILD_DATE

RUN apt-get update && apt-get install -y protobuf-compiler cmake

COPY . .
RUN cargo build --release

# --- STAGE 2: Runtime ---
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/sentiric-stream-gateway-service /usr/local/bin/
COPY static /app/static 

WORKDIR /app
ENV RUST_LOG=info
EXPOSE 18030
CMD ["sentiric-stream-gateway-service"]