# syntax=docker/dockerfile:1.7
FROM rust:1.97.1-bookworm@sha256:77fac8b98f9f46062bb680b6d25d5bcaabfc400143952ebc572e924bcbedc3fa AS builder

ARG STRAYLIGHT_BUILD_REVISION=unknown
ENV STRAYLIGHT_BUILD_REVISION=${STRAYLIGHT_BUILD_REVISION} \
    CARGO_BUILD_JOBS=2 \
    CARGO_INCREMENTAL=0
WORKDIR /build
COPY apps/api/Cargo.toml apps/api/Cargo.lock* ./
COPY apps/api/build.rs ./
COPY apps/api/src ./src
COPY apps/api/migrations ./migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo build --release \
    && cp /build/target/release/straylight /build/straylight \
    && cp /build/target/release/carrystate /build/carrystate

FROM builder AS test
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo test --lib

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:fccdbb0a547c14e23fcf4ce8ad62ca5d43b4faae8d22cd292f490fef9946c96e AS runtime
ARG STRAYLIGHT_BUILD_REVISION=unknown
LABEL org.opencontainers.image.title="Straylight API" \
      org.opencontainers.image.revision="${STRAYLIGHT_BUILD_REVISION}"
COPY --from=builder /build/straylight /usr/local/bin/straylight
COPY --from=builder /build/carrystate /usr/local/bin/carrystate

USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/straylight"]
CMD ["serve"]
