FROM golang:1.26.5-bookworm@sha256:1ecb7edf62a0408027bd5729dfd6b1b8766e578e8df93995b225dfd0944eb651 AS build

ARG MINIO_COMMIT=9e49d5e7a648f00e26f2246f4dc28e6b07f8c84a
ARG MINIO_RELEASE=RELEASE.2025-10-15T17-29-55Z

WORKDIR /src
RUN git clone --filter=blob:none https://github.com/minio/minio.git . \
    && git checkout --detach "${MINIO_COMMIT}" \
    && test "$(git rev-parse HEAD)" = "${MINIO_COMMIT}" \
    && test "$(git describe --tags --exact-match)" = "${MINIO_RELEASE}"
RUN make build

FROM alpine:3.23@sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40
ARG STRAYLIGHT_BUILD_REVISION=unknown
LABEL org.opencontainers.image.title="Straylight MinIO" \
      org.opencontainers.image.revision="${STRAYLIGHT_BUILD_REVISION}"
RUN apk add --no-cache ca-certificates \
    && addgroup -g 10001 -S straylight \
    && adduser -S -D -H -u 10001 -G straylight straylight
COPY --from=build /src/minio /usr/local/bin/minio
COPY entrypoint.sh /usr/local/bin/straylight-minio-entrypoint
EXPOSE 9000 9001
VOLUME ["/data"]
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/straylight-minio-entrypoint"]
