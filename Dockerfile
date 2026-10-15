# syntax=docker/dockerfile:1.7
FROM node:20-bookworm-slim AS console
WORKDIR /console
COPY console/package.json console/package-lock.json ./
RUN npm ci
COPY console/ .
RUN npm run build

FROM rust:1-bookworm AS build
ARG COMMIT=unknown
ARG BUILD_TIME
ENV ZANSHIN_BUILD_COMMIT=${COMMIT} ZANSHIN_BUILD_TIME=${BUILD_TIME}
WORKDIR /src
COPY . .
COPY --from=console /console/dist ./console/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p zanshin-server -p zanshin-cli && cp target/release/zanshin /zanshin && cp target/release/zanshinctl /zanshinctl

FROM debian:bookworm-slim
ARG VERSION=dev
ARG COMMIT=unknown
ARG BUILD_TIME
LABEL org.opencontainers.image.title="zanshin" \
      org.opencontainers.image.description="Zanshin LiveOps server" \
      org.opencontainers.image.source="https://github.com/rep0rtDev/zanshin" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${COMMIT}" \
      org.opencontainers.image.created="${BUILD_TIME}"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 zanshin
COPY --from=build /zanshin /usr/local/bin/zanshin
COPY --from=build /zanshinctl /usr/local/bin/zanshinctl
USER zanshin
EXPOSE 7550 7549
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s CMD curl -fsS http://localhost:7550/healthz || exit 1
ENTRYPOINT ["zanshin"]
