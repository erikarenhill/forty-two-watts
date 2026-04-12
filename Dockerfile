# Multi-stage build for home-ems
# Builds on host arch, cross-compiles if needed via --platform

FROM rust:1.86-bookworm AS builder

WORKDIR /build

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release 2>/dev/null || true && rm -rf src

# Build the actual app
COPY src/ src/
RUN cargo build --release

# Runtime stage — minimal image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary
COPY --from=builder /build/target/release/home-ems /app/home-ems

# Copy drivers and web UI
COPY drivers/ /app/drivers/
COPY web/ /app/web/
COPY config.example.yaml /app/

# Config volume mount point
VOLUME /app/data

EXPOSE 8080

ENTRYPOINT ["/app/home-ems"]
CMD ["/app/data/config.yaml"]
