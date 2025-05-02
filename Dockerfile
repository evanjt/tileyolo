FROM rust:1.86.0-slim AS chef
WORKDIR /app
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY Cargo.toml Cargo.lock /app/
COPY src /app/src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# install C/C++ toolchain, PROJ, GDAL deps, cmake, and sqlite3 CLI
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      build-essential \
      cmake \
      pkg-config \
      sqlite3 \
      proj-bin \
      libproj-dev \
      gdal-bin \
      libgdal-dev \
      libgeos-dev \
      libsqlite3-dev \
      libtiff-dev \
      libpng-dev \
      libjpeg-dev \
      libexpat1-dev \
      libxml2-dev && \
    apt-get clean && rm -rf /var/lib/apt/lists/*

COPY --from=planner /app/recipe.json /app/recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock /app/
COPY src /app/src
RUN cargo build --release --bin tileyolo

FROM debian:bookworm-slim AS runtime
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      openssl \
      ca-certificates \
      gdal-bin && \
    apt-get clean && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/tileyolo /usr/local/bin
ENTRYPOINT ["/usr/local/bin/tileyolo"]
