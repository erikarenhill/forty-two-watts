# Multi-stage build for forty-two-watts 🐬
# Uses musl for fully static binary — runs on any Linux without glibc dependency
FROM rust:latest AS builder
RUN rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl
RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
ARG TARGETARCH
RUN if [ "$TARGETARCH" = "arm64" ]; then \
      cargo build --release --target aarch64-unknown-linux-musl; \
      cp target/aarch64-unknown-linux-musl/release/forty-two-watts /build/forty-two-watts; \
    else \
      cargo build --release --target x86_64-unknown-linux-musl; \
      cp target/x86_64-unknown-linux-musl/release/forty-two-watts /build/forty-two-watts; \
    fi

# Runtime — alpine (tiny, musl-compatible)
FROM alpine:latest
WORKDIR /app
COPY --from=builder /build/forty-two-watts /app/forty-two-watts
COPY drivers/ /app/drivers/
COPY web/ /app/web/
VOLUME /app/data
EXPOSE 8080
ENTRYPOINT ["/app/forty-two-watts"]
CMD ["/app/data/config.yaml"]
