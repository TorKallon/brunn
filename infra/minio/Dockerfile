FROM golang:1.26.7-bookworm@sha256:e8c859f5632dcfde7b32d2012b4351728f6437930887c2f6a91ea242459e5514 AS build

ARG MINIO_COMMIT=9e49d5e7a648f00e26f2246f4dc28e6b07f8c84a
ARG MINIO_RELEASE=RELEASE.2025-10-15T17-29-55Z

WORKDIR /src
RUN git clone --filter=blob:none https://github.com/minio/minio.git . \
    && git checkout --detach "${MINIO_COMMIT}" \
    && test "$(git rev-parse HEAD)" = "${MINIO_COMMIT}" \
    && test "$(git describe --tags --exact-match)" = "${MINIO_RELEASE}"
RUN set -eu; \
    dependencies_ready=false; \
    for attempt in 1 2 3; do \
      if go get \
          github.com/apache/thrift@v0.24.0 \
          github.com/buger/jsonparser@v1.1.2 \
          github.com/go-jose/go-jose/v4@v4.1.4 \
          github.com/prometheus/prometheus@v0.311.3 \
          github.com/rabbitmq/amqp091-go@v1.13.0 \
          go.opentelemetry.io/otel/sdk@v1.44.0 \
          golang.org/x/crypto@v0.55.0 \
          golang.org/x/net@v0.57.0 \
          golang.org/x/text@v0.41.0 \
          google.golang.org/grpc@v1.83.1 \
        && go mod download all; then \
        dependencies_ready=true; \
        break; \
      fi; \
      sleep "$((attempt * 5))"; \
    done; \
    test "${dependencies_ready}" = true; \
    go mod tidy -e; \
    make build

FROM alpine:3.24@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b
ARG BRUNN_BUILD_REVISION=unknown
LABEL org.opencontainers.image.title="Brunn MinIO" \
      org.opencontainers.image.revision="${BRUNN_BUILD_REVISION}"
RUN apk upgrade --no-cache libcrypto3 libssl3 \
    && apk add --no-cache ca-certificates \
    && addgroup -g 10001 -S brunn \
    && adduser -S -D -H -u 10001 -G brunn brunn
COPY --from=build /src/minio /usr/local/bin/minio
COPY entrypoint.sh /usr/local/bin/brunn-minio-entrypoint
EXPOSE 9000 9001
VOLUME ["/data"]
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/brunn-minio-entrypoint"]
