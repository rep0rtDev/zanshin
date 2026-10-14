# syntax=docker/dockerfile:1.7
FROM node:20-bookworm-slim AS console
WORKDIR /console
COPY console/package.json console/package-lock.json ./
RUN npm ci
COPY console/ .
RUN npm run build

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
COPY --from=console /console/dist ./console/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p zanshin-server && cp target/release/zanshin /zanshin

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 zanshin
COPY --from=build /zanshin /usr/local/bin/zanshin
USER zanshin
EXPOSE 7550 7549
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s CMD curl -fsS http://localhost:7550/healthz || exit 1
ENTRYPOINT ["zanshin"]
