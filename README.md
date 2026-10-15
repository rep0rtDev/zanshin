<p align="center"><strong>残心 Zanshin</strong></p>
<p align="center">Open-source LiveOps backend for games.<br/>Feature flags · experiments · audiences · live events · in-app messages · analytics — self-hosted, one binary.</p>

---

Zanshin is an Apache-2.0 alternative to hosted LiveOps/personalisation services such as Satori. It
plugs into **Nakama** first-class, but is a standalone server with gRPC + REST APIs that any game
backend or client can talk to.

**Status:** feature-complete against the Satori surface and production-ready (two gap rounds,
release pipeline, deploy kit); see [docs/ROADMAP.md](docs/ROADMAP.md) for the full checklist and
the known gaps.

## Why

* **Own your player data.** Everything runs on your infrastructure: PostgreSQL as the system of record,
  Redis for caching and fan-out, ClickHouse (optional) for analytics at scale.
* **One binary.** Rust server, embedded migrations, embedded admin console, no sidecars.
* **Nakama-native.** A Go runtime module gives Nakama RPCs and hooks that evaluate flags, experiments and
  live events locally from a replicated cache — no extra network hop in the hot path.
* **Not Nakama-only.** gRPC and REST speak the same JSON/protobuf model; SDKs are generated from `proto/`.

## Quick start

```sh
git clone https://github.com/rep0rtDev/zanshin && cd zanshin
docker compose up -d
docker compose --profile demo up demo   # optional: sample flags/audiences/experiment/live event + 500 synthetic players
```

Mint a client API key with the bootstrap admin key from `docker-compose.yml`, then authenticate a player:

```sh
curl -s -X POST localhost:7550/admin/v1/api-keys \
  -H 'x-api-key: change-me-admin-key' -H 'content-type: application/json' \
  -d '{"name":"my-game","kind":"client"}'
# -> {"id":"…","kind":"client","secret":"zk_c_…"}

curl -s -X POST localhost:7550/v1/authenticate \
  -H 'x-api-key: zk_c_…' -H 'content-type: application/json' \
  -d '{"id":"player-42","default":{"platform":"android","country":"DE"},"custom":{"level":3}}'
# -> {"token":"…","refresh_token":"…","identity":{…},"properties":{"default":{…},"computed":{},"custom":{…}}}
```

gRPC is on `:7549` with server reflection enabled:

```sh
grpcurl -plaintext -H 'x-api-key: zk_c_…' -d '{"id":"player-42"}' localhost:7549 zanshin.api.v1.Zanshin/Authenticate
```

Run from source instead of Docker:

```sh
docker compose up -d postgres redis clickhouse
export ZANSHIN__AUTH__JWT_SECRET=$(openssl rand -hex 32)
export ZANSHIN__ADMIN__BOOTSTRAP_KEY=dev-admin-key-00000000
cargo run -p zanshin-server
```

Configuration is a YAML file (`config/zanshin.example.yml`) and/or `ZANSHIN__SECTION__KEY` environment
variables. `zanshin --check-config` prints the effective configuration with secrets redacted.

## What's in the box today

| Area | State |
|------|-------|
| Identities, sessions (JWT access + rotating refresh tokens, replay detection, revocation) | ✅ |
| Identity properties: `default` / `computed` / `custom` JSON namespaces | ✅ |
| API keys with `client` / `server` / `admin` scopes, bootstrap key | ✅ |
| REST (`/v1`, `/admin/v1`) and gRPC (`zanshin.api.v1.Zanshin`) with reflection + health | ✅ |
| PostgreSQL migrations, Redis cache, ClickHouse schema bootstrap | ✅ |
| Prometheus `/metrics`, structured logs, request ids, graceful shutdown | ✅ |
| Feature flags: typed values, targeting rules (`all`/`any`/`not`, 20 operators incl. semver), percentage rollouts, weighted variants, per-identity overrides, live push over WebSocket / gRPC stream, replicated via Redis | ✅ |
| Audiences: rule-based segments with sampling, stored membership (live mirror or scheduled recompute), member lists with join time, include / exclude lists with expiring exclusions, managed audiences pushed from the server API, usage references, copy, human-readable expression language, default audiences | ✅ |
| A/B experiments: targeting, allocation %, weighted variants, sticky assignment, exposure tracking, lifecycle draft→running→paused→stopped, results per variant, optional flag linkage | ✅ |
| Live events (schedules), in-app messages / inbox: templates (formats, translations, test send), cron / live-event schedules, webhook (HMAC) / OneSignal / Facebook / FCM / APNs channels with delivery log, inbox delete | ✅ |
| Webhooks: event-type subscriptions, HMAC-signed, batched, retried with backoff, delivery log with retry, test send | ✅ |
| Journeys: triggered per-player automations (messages, events, flag overrides, experiment assignment, integrations, waits incl. local time, branches) with persisted runs and step log | ✅ |
| Analytics v2: game-metrics overview (DAU/WAU/MAU, revenue, ARPDAU/ARPPU, sessions), retention cohorts, funnels, annotations, custom dashboards | ✅ |
| Analytics v3: status board, LiveOps calendar, country / platform / app-version / variant filters on every report, event explorer, player map, playtime, funnel models (unlimited / window / step / session), cumulative retention + installs, LTV / CPI / RoAS with ad-spend import, errors widget | ✅ |
| LiveOps polish: flag diff + payload sizes, `GET /v1/flags/overrides`, deterministic experiment precedence on a shared flag, rich template previews, skip-inbox / template-less direct messages, integration health, `$event_error` / `$identity_delete`, event size limits, player event search, per-key export / import, `409` on referenced delete, LiveOps recipes | ✅ |
| Console users: email + password (Argon2id), TOTP MFA with recovery codes, server-side sessions, `resource:action` permissions with presets (Admin / Developer / Game Designer / Debugger / Support / Analyst / Viewer), permission sets + rotation for admin keys | ✅ |
| Production: cluster-wide job locks, OpenTelemetry traces, per-route / gRPC / pool metrics, gRPC rate limits + timeouts, file secrets, `server.production` guard rails, event idempotency, retention config, deep `/readyz`, two-phase drain, load shedding | ✅ |
| Release + deploy: signed multi-arch images with SBOM / provenance, binaries, security scans, production Compose + Helm chart, Grafana dashboard + alerts, k6 load tests | ✅ |
| Data: export sinks to S3-compatible storage / HTTP / BigQuery (JSONL or CSV, gzip, cursor + run log), configuration bundles (export, diff, merge/replace import, import from another server), change history with revert and audit undo, GDPR takeout/delete, event purge, sample data generator | ✅ |
| Taxonomy: labels across every resource, event definitions with value types + metadata JSON Schema (off / warn / strict enforcement, auto-discovery, rejected-event debugger), read-only custom properties, JSON Schemas for structured flag values | ✅ |
| Player model: default properties enriched from request headers / geo-IP (platform, OS, device, app version, locale, language, timezone, country / region / city), `POST /v1/identify` to rename or merge an anonymous identity into an account (properties, events, assignments, inbox, devices move; merge history kept), play sessions with inactivity timeout, synthetic `$identity_create` / `$session_start` / `$session_end` / `$identify` / `$experiment_join` events | ✅ |
| Analytics events: client/server ingest, batching writer, ClickHouse sink (PostgreSQL fallback), events tagged with live experiment assignments, timeseries / top names / per-identity log, per-variant experiment metrics with conversion | ✅ |
| Computed properties maintained from events: `session_count`, `first/last_seen_at`, `event_count`, per-event `count` / `sum` / `first_at` / `last_at` / `high` / `low` / `last_value`, `play_time_sec` — targetable from rules | ✅ |
| Go SDK `sdk/go` (server-key client with local cache + change-stream invalidation) | ✅ |
| TypeScript SDK `sdk/ts` (browser + Node, zero deps): session with auto-refresh, flags, experiments, live events, inbox, events, WebSocket stream | ✅ |
| Admin console (`console/`, React, embedded in the binary at `/console`): visual rule builder, flags / experiments / audiences / live-event calendar / messages with preview / identities / analytics / API keys / audit log, dark theme | ✅ |
| Unreal C++ SDK `sdk/unreal` (plugin on engine HTTP/WebSockets/Json modules, Blueprint subsystem) | ✅ |
| Godot GDScript SDK `sdk/godot` (addon, HTTPRequest + WebSocketPeer, zero deps) | ✅ |
| Godot 3 GDScript SDK `sdk/godot3` (addon, HTTPRequest + WebSocketClient, zero deps) | ✅ |
| Defold Lua SDK `sdk/defold` (http + json, optional websocket extension) | ✅ |
| C++17 SDK `sdk/cpp` (header-only, bring-your-own transport, libcurl adapter) | ✅ |
| OpenAPI 3 spec + Swagger UI (`/docs`, `/openapi.json`) | ✅ |
| `zanshinctl` CLI (config export / import / diff / promote, keys, players, flags, sinks) | ✅ |
| Multi-project: isolated games / environments per deployment (schema + Redis + ClickHouse per project, `x-zanshin-project`, console switcher) | ✅ |
| Unity C# SDK `sdk/unity` (UPM, zero deps, netstandard2.1): same surface as the TS SDK incl. push-token registration and WebSocket stream | ✅ |
| Swift `sdk/swift`, Kotlin / JVM `sdk/kotlin`, Dart / Flutter `sdk/dart` SDKs (zero deps, full client surface, live stream) | ✅ |
| .NET server SDK `sdk/dotnet` (`Zanshin.Server`, netstandard2.0 / net8.0) with a change-stream-invalidated cache + `Zanshin.OpenFeature` provider | ✅ |
| Nakama runtime modules for Go (`nakama/`), JavaScript (`nakama/ts`) and Lua (`nakama/lua`) with identical RPCs; gRPC at full parity with REST | ✅ |
| Push notifications (FCM HTTP v1 / APNs token auth): device token registry, `push: true` on campaigns and direct messages, proactive campaign sweep | ✅ |

## API at a glance

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| `POST` | `/v1/authenticate` | client key | find-or-create identity, returns session |
| `POST` | `/v1/authenticate/refresh` | client key | rotate refresh token |
| `POST` | `/v1/authenticate/logout` | client key | revoke session |
| `GET` / `DELETE` | `/v1/identity` | session | current identity / delete it |
| `GET` / `PUT` | `/v1/properties` | session | read / merge properties (`null` removes a key) |
| `GET` | `/v1/flags[?names=a,b]`, `/v1/flags/{key}` | session | evaluate feature flags |
| `GET` | `/v1/audiences` | session | audiences the identity belongs to |
| `GET` / `POST` | `/v1/experiments[?names=]`, `/v1/experiments/{key}/expose` | session | enrolments (sticky) / record exposure |
| `POST` | `/v1/events` | session | report analytics events (batched) |
| `GET` | `/v1/live-events[?names=&upcoming=true]` | session | active (and upcoming) live events targeting the identity |
| `GET` / `PUT` / `DELETE` | `/v1/messages`, `/v1/messages/{id}` | session | inbox, mark read, consume |
| `WS` | `/v1/stream[?token=]` | session | live flag / audience / experiment changes pushed to the client |
| `GET` / `POST` / `PUT` | `/server/v1/{flags,audiences,experiments,live-events,messages/inbox}?external_id=`, `/server/v1/{identities,messages,events}`, `/server/v1/identities/{external_id}/properties` | server key | identity sync, evaluate for any identity, direct messages, events (Nakama / backends) |
| `WS` | `/server/v1/stream` | server key | change feed for backend caches |
| `GET` / `POST` | `/admin/v1/api-keys` | admin | list / mint API keys |
| `DELETE` | `/admin/v1/api-keys/{id}` | admin | revoke a key |
| `GET` | `/admin/v1/identities[?after=&limit=]` | admin | paginate identities |
| `GET` / `DELETE` | `/admin/v1/identities/{id}` | admin | inspect / delete |
| `PUT` | `/admin/v1/identities/{id}/properties` | admin | server-side property update |
| `GET` | `/admin/v1/identities/by-external-id/{xid}` | admin | lookup by external id |
| `*` | `/admin/v1/{flags,audiences,experiments,live-events,messages}…`, `/admin/v1/analytics/…` | admin | CRUD, overrides, lifecycle, results (+ event metrics), stats, analytics, per-identity and dry-run evaluation |
| `GET` | `/admin/v1/stats`, `/healthz`, `/readyz`, `/metrics` | — / admin | operations |

Full reference: [docs/api.md](docs/api.md). Design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Hardening

- **Rate limiting** — a Redis token bucket per caller and scope (`ip`, `client`, `session`, `server`,
  `admin`), configured under `limits:`; every API response carries `X-RateLimit-Limit`,
  `X-RateLimit-Remaining` and `X-RateLimit-Scope`, throttled calls get `429` + `Retry-After`.
  Any API key can carry its own budget (`rate_limit_per_minute` when minting it). Redis outages fail open.
- **Permissions** — every `/admin/v1` route requires a `resource:action` permission (`read` /
  `write` / `delete` over 18 resources, `*` with overrides); `403` answers carry
  `permission required: flags:write` and the console hides what the caller cannot reach. Presets
  Admin / Developer / Game Designer / Debugger / Support / Analyst / Viewer; the legacy roles
  `viewer` / `editor` / `admin` and `read_only: true` still map onto them.
- **Console users** — email + password (Argon2id, 12+ characters, forced change of issued
  passwords), TOTP MFA with recovery codes, server-side sessions with revoke / sign-out-everywhere,
  admin-issued reset links, first-admin bootstrap with the bootstrap key.
- **Console SSO** — `admin.oidc` enables login through any OpenID Connect provider (authorization
  code + PKCE, ID token verified against the provider JWKS, nonce/state bound to a signed cookie).
  A claim (`role_claim`, default `groups`) is mapped to a role via `role_map`, optionally restricted
  by `allowed_domains`; the console session is an HttpOnly cookie valid `console_session_ttl`.
  Audit entries record the user's email with `actor_kind: oidc`.
- **API key scopes** — admin keys can be minted `read_only` (GET-only on `/admin/v1`), any key can have
  an `expires_at`; both are enforced at authentication time.
- **Audit log** — every mutating `/admin/v1` call (including rejected and unauthenticated attempts) is
  recorded with actor, method, path, status, client IP, request id and a redacted JSON body; browse it
  with `GET /admin/v1/audit` (filters: `actor`, `method`, `path`, `since`, cursor `after`). Retention via
  `audit.retention`.
- **Headers** — `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy`, optional
  HSTS (`server.hsts`), a strict CSP on the console; `server.trust_proxy` controls whether
  `X-Forwarded-For` is believed.

## Install

- **Container**: `ghcr.io/rep0rtdev/zanshin:<version>` (linux/amd64, linux/arm64; signed, with SBOM
  and provenance attestations — see [docs/releasing.md](docs/releasing.md) for verification).
- **Binaries**: Linux / macOS / Windows archives on the
  [Releases](https://github.com/rep0rtDev/zanshin/releases) page, with signed `SHA256SUMS`.
- **Nakama**: `ghcr.io/rep0rtdev/zanshin-nakama:<version>-nakama<nakama version>` bundles the
  runtime module into the official Nakama image.
- `zanshin --version` and `GET /admin/v1/system` (`build`) report the exact commit, build time
  and toolchain of a running server.
- **Deploy**: `deploy/compose` (single host, Caddy TLS, backups) and `deploy/helm/zanshin`
  (Kubernetes); dashboards, alerts and k6 load tests under `deploy/`. Operator guide:
  [docs/production.md](docs/production.md).

## Running in production

* **Replicas**: run as many `zanshin` processes as you like behind a load balancer. Catalogs replicate
  through Redis pub/sub; housekeeping jobs (sweeps, recomputes, purges, exports) take a cluster-wide
  Postgres advisory lock, so each job runs once per tick and fails over when a replica dies.
* **Rollouts**: `zanshin --migrate` applies migrations and exits (init container / release step);
  `SIGTERM` starts a drain (`/readyz` → 503 for `server.shutdown_grace`, listeners stay open), then
  listeners close and in-flight requests / WebSocket streams get `server.shutdown_timeout` to finish.
* **Probes**: `/healthz` is liveness; `/readyz` probes PostgreSQL, Redis and ClickHouse with latency
  and pool stats in the body and answers 503 (with the failing component) when any is down.
* **Metrics**: Prometheus at `/metrics` — per-route `zanshin_http_request_seconds` / `_requests_total`,
  per-method `zanshin_grpc_*`, `zanshin_db_pool_connections`, `zanshin_event_queue_depth`,
  `zanshin_job_seconds` plus every domain counter.
* **Traces**: set `telemetry.otlp_endpoint` (OTLP/gRPC: Tempo, Jaeger, Honeycomb, Datadog agent…).
  Logs carry the `request_id` of every span; `log.json: true` for one JSON object per line.
* **Secrets**: any key can come from a file with the `ZANSHIN_FILE__` prefix
  (`ZANSHIN_FILE__AUTH__JWT_SECRET=/run/secrets/jwt`). `server.production: true` refuses to start
  with placeholder secrets, open CORS, anonymous authenticate or a non-TLS database URL.
* **Limits**: HTTP and gRPC share the same token buckets (`limits.*`), body/message size caps and
  request timeouts; gRPC adds a per-process concurrency cap and HTTP/2 keepalives.
* **Retention**: `retention.*` controls every purge (events, export runs, config history, journey
  runs, webhook deliveries, batched deletes); `clickhouse.retention_days` sets the ClickHouse TTL.
* **Idempotency**: client event ids are remembered for 24h across replicas, so SDK retries never
  double count.

## Admin console

Open `http://localhost:7550/console/` and sign in with the bootstrap admin key (or any admin
API key), or with SSO when `admin.oidc` is configured — see `config/zanshin.example.yml`. The UI is a React app in [`console/`](console) compiled into the server binary — run
`npm ci && npm run build` there before `cargo build` (the Dockerfile does this for you).

The console is organised into Targeting (flags, audiences, experiments, live events), Engage
(messages, templates, journeys, integrations, recipes), Players, Analytics (insights, status board,
calendar, explorer) and Platform (webhooks, data, users, API keys, audit log, settings). `⌘K` / `Ctrl+K` opens a command palette that jumps to any page or
entity; lists have search, status / label filters, sorting, pagination and a per-row menu
(open / duplicate / delete); detail pages have tabs — Settings, entity-specific panels, History
(every recorded change, with one-click revert) and Activity (audit entries for that entity) —
with `⌘S` to save and an unsaved-changes guard. Creating a flag walks through a short wizard
(basics → value → targeting → review). Viewers see everything read-only; `Settings` shows server
version, stores, auth, rate limits and an inventory (`GET /admin/v1/system`).

## SDKs

End-to-end example with Nakama and Unity: [rep0rtDev/zanshin-unity-example](https://github.com/rep0rtDev/zanshin-unity-example).

- [`sdk/ts`](sdk/ts) — TypeScript for browsers and Node: `authenticate` / `restore`, automatic
  token refresh, `flags`, `experiments` + `expose`, `liveEvents`, `inbox`, `track`, and a
  reconnecting WebSocket stream (`z.stream().onFlag(...)`).
- [`sdk/unity`](sdk/unity) — Unity 2021.3+ UPM package (`https://github.com/rep0rtDev/zanshin.git?path=sdk/unity`):
  async `ZanshinClient` with auto refresh, flags / experiments / live events / inbox / push tokens /
  events, `ZanshinStream` over `System.Net.WebSockets`.
- [`sdk/dotnet`](sdk/dotnet) — server-side .NET (`Zanshin.Server`, netstandard2.0 / net8.0): flags / experiments /
  audiences / live events / inbox / events / messages by external id, cache dropped by the change stream;
  `Zanshin.OpenFeature` provider on top.
- [`sdk/unreal`](sdk/unreal) — Unreal Engine 5.3+ plugin: `FZanshinClient` (async, auto refresh, all client
  endpoints), `FZanshinStream` over the engine `WebSockets` module, Blueprint-friendly `UZanshinSubsystem`.
- [`sdk/godot`](sdk/godot) — Godot 4.3+ GDScript addon (`Zanshin` autoload): awaitable client with auto refresh,
  typed helpers, `WebSocketPeer` live stream; headless smoke test against a live server.
- [`sdk/godot3`](sdk/godot3) — Godot 3.5+ GDScript addon with the same surface (`yield(..., "completed")` coroutines,
  `WebSocketClient` stream).
- [`sdk/defold`](sdk/defold) — Defold Lua module: callback API over `http`/`json`, auto refresh, live stream through
  extension-websocket.
- [`sdk/cpp`](sdk/cpp) — header-only C++17 client (`zanshin::Client`, `Result<T>`, own JSON) with a pluggable `HttpTransport`
  and a libcurl adapter; for Cocos2d-x and custom engines.
- `zanshinctl` (`crates/zanshin-cli`, shipped with the release binaries) — `config export|diff|import|promote`, `keys`,
  `player get|events|flags|delete`, `flags set`, `sinks run`, raw `api` calls; configured via `ZANSHIN_URL` / `ZANSHIN_ADMIN_KEY`.
- Interactive API reference at `/docs` (Swagger UI) backed by `/openapi.json` / `/openapi.yaml` (`docs/openapi.yaml`).
- [`sdk/swift`](sdk/swift) — Swift Package (iOS / macOS / tvOS / watchOS / visionOS / Linux): `actor ZanshinClient` with
  `async`/`await`, `ZanshinStream` over `URLSessionWebSocketTask`, zero dependencies.
- [`sdk/kotlin`](sdk/kotlin) — Kotlin / JVM (Android, servers): JDK 11 `HttpClient` + `WebSocket`, built-in JSON,
  blocking calls plus `CompletableFuture` twins.
- [`sdk/dart`](sdk/dart) — Dart / Flutter: `dart:io` client with the same surface and a broadcast `Stream<StreamMessage>`.
- [`sdk/go`](sdk/go) — Go for trusted backends: server-key client with a per-identity evaluation
  cache invalidated by the `/server/v1/stream` change feed, identity sync, direct messages, events.
- [`nakama/`](nakama) — Nakama runtime plugin on top of the Go SDK: users become Zanshin
  identities on login; clients call `zanshin.flags`, `zanshin.experiments`, `zanshin.live_events`,
  `zanshin.inbox`, `zanshin.track`… as regular RPCs. Build with `docker build -f nakama/Dockerfile .`.
  The same module exists for the JavaScript runtime ([`nakama/ts`](nakama/ts), single-file build) and the Lua
  runtime ([`nakama/lua`](nakama/lua)); all three expose identical RPC names and payloads.

## License

Apache-2.0. See [LICENSE](LICENSE).
