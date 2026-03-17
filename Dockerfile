# Build stage
FROM rust:1.89-bookworm AS builder

RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    libprotobuf-dev \
    libzmq3-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

ENV PROTOC_INCLUDE=/usr/include
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    libzmq5 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /etc/rak-basicstation /var/lib/rak-basicstation/credentials

COPY --from=builder /build/target/release/rak-basicstation /usr/bin/rak-basicstation
COPY packaging/docker/rak-basicstation.toml /etc/rak-basicstation/rak-basicstation.toml
COPY packaging/docker/docker-entrypoint.sh /usr/bin/docker-entrypoint.sh

EXPOSE 1700/udp

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["rak-basicstation", "-c", "/etc/rak-basicstation/rak-basicstation.toml"]
