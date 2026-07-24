FROM rust:1.96.1-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663 AS builder

ARG STRAYLIGHT_BUILD_REVISION=unknown
ENV STRAYLIGHT_BUILD_REVISION=${STRAYLIGHT_BUILD_REVISION}
WORKDIR /build
COPY apps/api/Cargo.toml apps/api/Cargo.lock* ./
COPY apps/api/build.rs ./
COPY apps/api/src ./src
COPY apps/api/migrations ./migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release \
    && cp /build/target/release/straylight /build/straylight

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:fccdbb0a547c14e23fcf4ce8ad62ca5d43b4faae8d22cd292f490fef9946c96e AS runtime
ARG STRAYLIGHT_BUILD_REVISION=unknown
LABEL org.opencontainers.image.title="Straylight API" \
      org.opencontainers.image.revision="${STRAYLIGHT_BUILD_REVISION}"
COPY --from=builder /build/straylight /usr/local/bin/straylight

USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/straylight"]
CMD ["serve"]
