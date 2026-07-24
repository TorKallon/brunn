FROM golang:1.26.5-alpine3.23@sha256:622e56dbc11a8cfe87cafa2331e9a201877271cbff918af53d3be315f3da88cc AS build

ARG CADDY_COMMIT=e2eee6a7fce366321294c9c2a79f3146891dcbdf
ARG CADDY_RELEASE=v2.11.4
RUN apk add --no-cache git
WORKDIR /src
RUN git clone --filter=blob:none https://github.com/caddyserver/caddy.git . \
    && git checkout --detach "${CADDY_COMMIT}" \
    && test "$(git rev-parse HEAD)" = "${CADDY_COMMIT}" \
    && test "$(git describe --tags --exact-match)" = "${CADDY_RELEASE}"
RUN go get google.golang.org/grpc@v1.82.1 \
    && go mod tidy \
    && CGO_ENABLED=0 go build \
      -trimpath \
      -ldflags="-s -w" \
      -o /out/caddy \
      ./cmd/caddy

FROM alpine:3.23@sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40
ARG STRAYLIGHT_BUILD_REVISION=unknown
LABEL org.opencontainers.image.title="Straylight Caddy Edge" \
      org.opencontainers.image.revision="${STRAYLIGHT_BUILD_REVISION}"
RUN apk add --no-cache ca-certificates \
    && addgroup -g 10001 -S straylight \
    && adduser -S -D -H -u 10001 -G straylight straylight \
    && install -d -m 0555 /etc/caddy
COPY --from=build /out/caddy /usr/local/bin/caddy
COPY --chmod=0444 Caddyfile /etc/caddy/Caddyfile

USER 10001:10001
EXPOSE 80 443 443/udp
ENTRYPOINT ["/usr/local/bin/caddy"]
CMD ["run", "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"]
