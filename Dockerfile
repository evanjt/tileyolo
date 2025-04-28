FROM rust:1.86.0-slim AS chef
WORKDIR /app
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY ./src /app/src
COPY Cargo.lock Cargo.toml /app/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

# Install GDAL, pkg-config, and dependencies for gdal-sys
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        pkg-config \
        gdal-bin \
        libgdal-dev \
        libproj-dev \
        libgeos-dev \
        libsqlite3-dev \
        libtiff-dev \
        libpng-dev \
        libjpeg-dev \
        libexpat1-dev \
        libxml2-dev \
        && apt-get clean && rm -rf /var/lib/apt/lists/*

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build application
COPY ./src /app/src
COPY Cargo.lock Cargo.toml /app/
RUN cargo build --release --bin tileyolo



# Runtime stage
FROM debian:bookworm-slim AS runtime
RUN apt-get update && \
    apt-get install -y --no-install-recommends openssl ca-certificates gdal-bin && \
    apt-get clean && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/tileyolo /usr/local/bin
ENTRYPOINT ["/usr/local/bin/tileyolo"]
