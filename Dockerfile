FROM rust:1.85.0 AS chef
WORKDIR /app
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY ./src /app/src
COPY Cargo.lock Cargo.toml /app/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN apt-get update && apt-get install -y --no-install-recommends gdal-bin && apt-get clean && rm -rf /var/lib/apt/lists/*
RUN cargo chef cook --release --recipe-path recipe.json

# Build application
COPY ./src /app/src
COPY Cargo.lock Cargo.toml /app/

RUN cargo build --release --bin spice-api

# We do not need the Rust toolchain to run the binary!
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends openssl ca-certificates gdal-bin && apt-get clean && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/tileyolo /usr/local/bin
ENTRYPOINT ["/usr/local/bin/tileyolo"]
