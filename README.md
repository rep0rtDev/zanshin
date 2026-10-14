<p align="center"><strong>残心 Zanshin</strong></p>
<p align="center">Open-source LiveOps backend for games.<br/>Feature flags · experiments · audiences · live events · in-app messages · analytics — self-hosted, one binary.</p>

---

Zanshin is an Apache-2.0 alternative to hosted LiveOps/personalisation services such as Satori. It
plugs into **Nakama** first-class, but is a standalone server with gRPC + REST APIs that any game
backend or client can talk to.

**Status:** early core. The identity / session layer and the operational skeleton are in place; the
LiveOps features are landing module by module (see [docs/ROADMAP.md](docs/ROADMAP.md)).

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
| Audiences (rule-based segments with sampling) gating flags and experiments | ✅ |
| A/B experiments: targeting, allocation %, weighted variants, sticky assignment, exposure tracking, lifecycle draft→running→paused→stopped, results per variant, optional flag linkage | ✅ |
| Live events (schedules), in-app messages / inbox | 🔜 |
| Analytics events: client/server ingest, batching writer, ClickHouse sink (PostgreSQL fallback), events tagged with live experiment assignments, timeseries / top names / per-identity log, per-variant experiment metrics with conversion | ✅ |
| Computed properties maintained from events: `session_count`, `first/last_seen_at`, `event_count`, per-event `count` / `sum` / `last_at` — targetable from rules | ✅ |
| Go SDK `sdk/go` (server-key client with local cache + change-stream invalidation) | ✅ |
| TypeScript SDK `sdk/ts` (browser + Node, zero deps): session with auto-refresh, flags, experiments, live events, inbox, events, WebSocket stream | ✅ |
| Admin console (`console/`, React, embedded in the binary at `/console`): visual rule builder, flags / experiments / audiences / live-event calendar / messages with preview / identities / analytics / API keys / audit log, dark theme | ✅ |
| Unreal C++ SDK `sdk/unreal` (plugin on engine HTTP/WebSockets/Json modules, Blueprint subsystem) | ✅ |
| Godot GDScript SDK `sdk/godot` (addon, HTTPRequest + WebSocketPeer, zero deps) | ✅ |
| Unity C# SDK `sdk/unity` (UPM, zero deps, netstandard2.1): same surface as the TS SDK incl. push-token registration and WebSocket stream | ✅ |
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
- **API key scopes** — admin keys can be minted `read_only` (GET-only on `/admin/v1`), any key can have
  an `expires_at`; both are enforced at authentication time.
- **Audit log** — every mutating `/admin/v1` call (including rejected and unauthenticated attempts) is
  recorded with actor, method, path, status, client IP, request id and a redacted JSON body; browse it
  with `GET /admin/v1/audit` (filters: `actor`, `method`, `path`, `since`, cursor `after`). Retention via
  `audit.retention`.
- **Headers** — `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy`, optional
  HSTS (`server.hsts`), a strict CSP on the console; `server.trust_proxy` controls whether
  `X-Forwarded-For` is believed.

## Admin console

Open `http://localhost:7550/console/` and sign in with the bootstrap admin key (or any admin
API key). The UI is a React app in [`console/`](console) compiled into the server binary — run
`npm ci && npm run build` there before `cargo build` (the Dockerfile does this for you).

## SDKs

- [`sdk/ts`](sdk/ts) — TypeScript for browsers and Node: `authenticate` / `restore`, automatic
  token refresh, `flags`, `experiments` + `expose`, `liveEvents`, `inbox`, `track`, and a
  reconnecting WebSocket stream (`z.stream().onFlag(...)`).
- [`sdk/unity`](sdk/unity) — Unity 2021.3+ UPM package (`https://github.com/rep0rtDev/zanshin.git?path=sdk/unity`):
  async `ZanshinClient` with auto refresh, flags / experiments / live events / inbox / push tokens /
  events, `ZanshinStream` over `System.Net.WebSockets`.
- [`sdk/unreal`](sdk/unreal) — Unreal Engine 5.3+ plugin: `FZanshinClient` (async, auto refresh, all client
  endpoints), `FZanshinStream` over the engine `WebSockets` module, Blueprint-friendly `UZanshinSubsystem`.
- [`sdk/godot`](sdk/godot) — Godot 4.3+ GDScript addon (`Zanshin` autoload): awaitable client with auto refresh,
  typed helpers, `WebSocketPeer` live stream; headless smoke test against a live server.
- [`sdk/go`](sdk/go) — Go for trusted backends: server-key client with a per-identity evaluation
  cache invalidated by the `/server/v1/stream` change feed, identity sync, direct messages, events.
- [`nakama/`](nakama) — Nakama runtime plugin on top of the Go SDK: users become Zanshin
  identities on login; clients call `zanshin.flags`, `zanshin.experiments`, `zanshin.live_events`,
  `zanshin.inbox`, `zanshin.track`… as regular RPCs. Build with `docker build -f nakama/Dockerfile .`.

## License

Apache-2.0. See [LICENSE](LICENSE).
