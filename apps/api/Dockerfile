FROM rust:1.96.1-bookworm AS builder

WORKDIR /build
COPY apps/api/Cargo.toml apps/api/Cargo.lock* ./
COPY apps/api/build.rs ./
COPY apps/api/src ./src
COPY apps/api/migrations ./migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release \
    && cp /build/target/release/straylight /build/straylight

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 straylight
COPY --from=builder /build/straylight /usr/local/bin/straylight

USER straylight
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/straylight"]
CMD ["serve"]
