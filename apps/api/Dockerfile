# syntax=docker/dockerfile:1.7
FROM rust:1.96.1-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663 AS builder

ARG BRUNN_BUILD_REVISION=unknown
ENV BRUNN_BUILD_REVISION=${BRUNN_BUILD_REVISION} \
    CARGO_BUILD_JOBS=2 \
    CARGO_INCREMENTAL=0
WORKDIR /build
COPY apps/api/Cargo.toml apps/api/Cargo.lock* ./
COPY apps/api/build.rs ./
COPY apps/api/src ./src
COPY apps/api/migrations ./migrations
# Railway cache-mount IDs are service-scoped and cannot interpolate the
# current service ID. This Dockerfile is shared by the API and worker, so rely
# on ordinary image-layer caching instead of coupling it to either service.
RUN cargo build --release \
    && cp /build/target/release/brunn /build/brunn \
    && cp /build/target/release/brunn-state /build/brunn-state

FROM builder AS test
RUN cargo test --lib

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:fccdbb0a547c14e23fcf4ce8ad62ca5d43b4faae8d22cd292f490fef9946c96e AS runtime
ARG BRUNN_BUILD_REVISION=unknown
LABEL org.opencontainers.image.title="Brunn API" \
      org.opencontainers.image.revision="${BRUNN_BUILD_REVISION}"
COPY --from=builder /build/brunn /usr/local/bin/brunn
COPY --from=builder /build/brunn-state /usr/local/bin/brunn-state

USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/brunn"]
CMD ["serve"]
