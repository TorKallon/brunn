FROM node:24.18.0-bookworm-slim@sha256:6f7b03f7c2c8e2e784dcf9295400527b9b1270fd37b7e9a7285cf83b6951452d AS build

WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY tsconfig.json ./
COPY src ./src
RUN npm run build

FROM build AS production-dependencies
RUN rm -rf node_modules \
    && npm ci --omit=dev \
    && npm cache clean --force

FROM gcr.io/distroless/nodejs24-debian13:nonroot@sha256:af85d11ce7ef10172855a6e3649e3e8125b1b9e3ca41849ec2918036f05cb212 AS runtime

ARG BRUNN_BUILD_REVISION=unknown
LABEL org.opencontainers.image.title="Brunn MCP Adapter" \
      org.opencontainers.image.revision="${BRUNN_BUILD_REVISION}"
ENV NODE_ENV=production
WORKDIR /app
COPY --from=production-dependencies /app/package.json ./
COPY --from=production-dependencies /app/node_modules ./node_modules
COPY --from=build /app/dist ./dist

USER nonroot:nonroot
ENTRYPOINT ["/nodejs/bin/node", "dist/index.js"]
