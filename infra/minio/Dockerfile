FROM golang:1.24.8-bookworm AS build

ARG MINIO_COMMIT=9e49d5e7a648f00e26f2246f4dc28e6b07f8c84a
ARG MINIO_RELEASE=RELEASE.2025-10-15T17-29-55Z

WORKDIR /src
RUN git clone --filter=blob:none https://github.com/minio/minio.git . \
    && git checkout --detach "${MINIO_COMMIT}" \
    && test "$(git rev-parse HEAD)" = "${MINIO_COMMIT}" \
    && test "$(git describe --tags --exact-match)" = "${MINIO_RELEASE}"
RUN make build

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/minio /usr/local/bin/minio
EXPOSE 9000 9001
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/minio"]

