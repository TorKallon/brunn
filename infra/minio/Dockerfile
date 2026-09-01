FROM golang:1.25.12-bookworm@sha256:ea341baa9bd5ba6784f6d7161ace70544349a6242d54d34a0fbfd2c4d51c9d58 AS build

ARG MINIO_COMMIT=9e49d5e7a648f00e26f2246f4dc28e6b07f8c84a
ARG MINIO_RELEASE=RELEASE.2025-10-15T17-29-55Z

WORKDIR /src
RUN git clone --filter=blob:none https://github.com/minio/minio.git . \
    && git checkout --detach "${MINIO_COMMIT}" \
    && test "$(git rev-parse HEAD)" = "${MINIO_COMMIT}" \
    && test "$(git describe --tags --exact-match)" = "${MINIO_RELEASE}"
RUN make build

FROM alpine:3.23@sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40
ARG BRUNN_BUILD_REVISION=unknown
LABEL org.opencontainers.image.title="Brunn MinIO" \
      org.opencontainers.image.revision="${BRUNN_BUILD_REVISION}"
RUN apk add --no-cache ca-certificates \
    && addgroup -g 10001 -S brunn \
    && adduser -S -D -H -u 10001 -G brunn brunn
COPY --from=build /src/minio /usr/local/bin/minio
COPY entrypoint.sh /usr/local/bin/brunn-minio-entrypoint
EXPOSE 9000 9001
VOLUME ["/data"]
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/brunn-minio-entrypoint"]
