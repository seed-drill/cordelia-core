# Build stage
FROM rust:latest AS builder

WORKDIR /build
COPY . .

RUN cargo clean && cargo build --release --bin cordelia-node

# Runtime stage
FROM debian:bookworm-slim

ARG BOOT_CONFIG=boot1-config.toml

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -s /bin/bash cordelia
WORKDIR /home/cordelia

COPY --from=builder /build/target/release/cordelia-node /usr/local/bin/cordelia-node
COPY ${BOOT_CONFIG} /home/cordelia/default-config.toml

RUN mkdir -p /home/cordelia/.cordelia && chown -R cordelia:cordelia /home/cordelia
USER cordelia

EXPOSE 9474/tcp
EXPOSE 9473/tcp

# Copy default config to volume mount if not already present, then start
CMD ["sh", "-c", "test -f /home/cordelia/.cordelia/config.toml || cp /home/cordelia/default-config.toml /home/cordelia/.cordelia/config.toml; exec cordelia-node --config /home/cordelia/.cordelia/config.toml"]
