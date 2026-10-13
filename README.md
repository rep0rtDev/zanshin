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
* **One binary.** Rust server, embedded migrations, embedded admin console (coming), no sidecars.
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
| Analytics event ingest (ClickHouse, PostgreSQL fallback), computed properties | 🔜 |
| Go SDK + Nakama runtime module, TypeScript / Unity / Unreal / Godot SDKs | 🔜 |
| Embedded admin console | 🔜 |

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
| `WS` | `/v1/stream[?token=]` | session | live flag / audience / experiment changes pushed to the client |
| `GET` | `/server/v1/{flags,audiences,experiments}?external_id=` | server key | evaluate for any identity (Nakama / backends) |
| `GET` / `POST` | `/admin/v1/api-keys` | admin | list / mint API keys |
| `DELETE` | `/admin/v1/api-keys/{id}` | admin | revoke a key |
| `GET` | `/admin/v1/identities[?after=&limit=]` | admin | paginate identities |
| `GET` / `DELETE` | `/admin/v1/identities/{id}` | admin | inspect / delete |
| `PUT` | `/admin/v1/identities/{id}/properties` | admin | server-side property update |
| `GET` | `/admin/v1/identities/by-external-id/{xid}` | admin | lookup by external id |
| `*` | `/admin/v1/{flags,audiences,experiments}…` | admin | CRUD, overrides, lifecycle, results, per-identity and dry-run evaluation |
| `GET` | `/admin/v1/stats`, `/healthz`, `/readyz`, `/metrics` | — / admin | operations |

Full reference: [docs/api.md](docs/api.md). Design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
