# syntax=docker/dockerfile:1.7
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked \
    && cp target/release/s3-cdn-network /usr/local/bin/s3-cdn-network

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -s /usr/sbin/nologin cdnuser
COPY --from=builder /usr/local/bin/s3-cdn-network /usr/local/bin/s3-cdn-network
USER cdnuser
EXPOSE 9876
ENV RUST_LOG=s3_cdn_network=info
CMD ["s3-cdn-network"]
