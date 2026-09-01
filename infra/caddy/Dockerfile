FROM golang:1.26.6-alpine3.23@sha256:e57c41c1d5864341031181b0db34b9a537bb5773eb6428e4e5bdaea0f9135406 AS build

ARG CADDY_COMMIT=e2eee6a7fce366321294c9c2a79f3146891dcbdf
ARG CADDY_RELEASE=v2.11.4
RUN apk add --no-cache git
WORKDIR /src
RUN git clone --filter=blob:none https://github.com/caddyserver/caddy.git . \
    && git checkout --detach "${CADDY_COMMIT}" \
    && test "$(git rev-parse HEAD)" = "${CADDY_COMMIT}" \
    && test "$(git describe --tags --exact-match)" = "${CADDY_RELEASE}"
RUN go get golang.org/x/crypto@v0.55.0 \
      golang.org/x/net@v0.57.0 \
      golang.org/x/text@v0.41.0 \
      google.golang.org/grpc@v1.82.1 \
    && go mod tidy \
    && CGO_ENABLED=0 go build \
      -trimpath \
      -ldflags="-s -w" \
      -o /out/caddy \
      ./cmd/caddy

FROM alpine:3.23@sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40
ARG BRUNN_BUILD_REVISION=unknown
LABEL org.opencontainers.image.title="Brunn Caddy Edge" \
      org.opencontainers.image.revision="${BRUNN_BUILD_REVISION}"
RUN apk upgrade --no-cache libcrypto3 libssl3 \
    && apk add --no-cache ca-certificates \
    && addgroup -g 10001 -S brunn \
    && adduser -S -D -H -u 10001 -G brunn brunn \
    && install -d -m 0555 /etc/caddy
COPY --from=build /out/caddy /usr/local/bin/caddy
COPY --chmod=0444 Caddyfile /etc/caddy/Caddyfile

USER 10001:10001
EXPOSE 80 443 443/udp
ENTRYPOINT ["/usr/local/bin/caddy"]
CMD ["run", "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"]
