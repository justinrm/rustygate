FROM rust:1-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations

RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && groupadd --system rustygate \
    && useradd --system --gid rustygate --home-dir /nonexistent --shell /usr/sbin/nologin rustygate \
    && mkdir -p /data \
    && chown rustygate:rustygate /data \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/rustygate /usr/local/bin/rustygate
COPY config ./config
COPY .env.example ./

ENV RUSTYGATE_CONFIG=config/gateway.example.toml
VOLUME ["/data"]
EXPOSE 8080

USER rustygate

CMD ["rustygate"]
