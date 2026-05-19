FROM rust:1.83-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies before sources for faster incremental builds.
COPY Cargo.toml rust-toolchain.toml ./
COPY crates/common/Cargo.toml crates/common/Cargo.toml
COPY crates/gateway/Cargo.toml crates/gateway/Cargo.toml
COPY crates/cli/Cargo.toml crates/cli/Cargo.toml
RUN mkdir -p crates/common/src crates/gateway/src crates/cli/src \
    && echo "fn main() {}" | tee crates/gateway/src/main.rs crates/cli/src/main.rs > /dev/null \
    && echo "" > crates/common/src/lib.rs \
    && cargo build --release --bin portex-gateway --bin portex \
    && rm -rf crates/common/src crates/gateway/src crates/cli/src

COPY crates/ crates/
RUN touch crates/common/src/lib.rs crates/gateway/src/main.rs crates/cli/src/main.rs \
    && cargo build --release --bin portex-gateway --bin portex

FROM debian:bookworm-slim AS gateway
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/portex-gateway /usr/local/bin/portex-gateway
EXPOSE 8080/tcp 4443/udp
ENTRYPOINT ["/usr/local/bin/portex-gateway"]
