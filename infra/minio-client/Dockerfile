FROM golang:1.25.12-bookworm@sha256:ea341baa9bd5ba6784f6d7161ace70544349a6242d54d34a0fbfd2c4d51c9d58 AS build

ARG MC_COMMIT=7394ce0dd2a80935aded936b09fa12cbb3cb8096
ARG MC_RELEASE=RELEASE.2025-08-13T08-35-41Z

WORKDIR /src
RUN git clone --filter=blob:none https://github.com/minio/mc.git . \
    && git checkout --detach "${MC_COMMIT}" \
    && test "$(git rev-parse HEAD)" = "${MC_COMMIT}" \
    && test "$(git describe --tags --exact-match)" = "${MC_RELEASE}"
RUN go get \
      github.com/prometheus/prometheus@v0.311.3 \
      golang.org/x/crypto@v0.52.0 \
      golang.org/x/net@v0.55.0 \
      google.golang.org/grpc@v1.82.1 \
    && go mod tidy \
    && make build

FROM alpine:3.23@sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40
ARG BRUNN_BUILD_REVISION=unknown
LABEL org.opencontainers.image.title="Brunn MinIO Client" \
      org.opencontainers.image.revision="${BRUNN_BUILD_REVISION}"
RUN apk add --no-cache ca-certificates \
    && addgroup -g 10001 -S brunn \
    && adduser -S -D -h /home/brunn -u 10001 -G brunn brunn \
    && install -d -o 10001 -g 10001 -m 0700 /home/brunn
COPY --from=build /src/mc /usr/local/bin/mc
ENV HOME=/home/brunn
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/mc"]
