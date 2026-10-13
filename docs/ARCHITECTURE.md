# Architecture

## Goals

1. **Single binary, boring operations.** `zanshin` embeds migrations and (later) the admin console. The
   only hard dependencies are PostgreSQL and Redis; ClickHouse is optional and only used for analytics.
2. **Hot path is local.** Flag / audience / experiment evaluation happens from an in-memory model that is
   replicated through Redis pub/sub to every server replica and to the Nakama module. Client reads do not
   touch PostgreSQL.
3. **One domain, two transports.** REST and gRPC are thin adapters over `zanshin_server::services`; both
   map the same `zanshin_core::Error` to status codes in one place.
4. **Nakama-first, not Nakama-only.** Protobuf is the source of truth for every SDK.

## Crates

```
crates/
  zanshin-core    domain types, validation, token issuing, property merge rules — no IO
  zanshin-store   PostgreSQL repositories (sqlx), Redis cache/bus, ClickHouse sink
  zanshin-proto   .proto → tonic/prost code, file descriptor set for reflection
  zanshin-server  binary: config, telemetry, services, REST (axum), gRPC (tonic), housekeeping
```

Dependencies point downwards only: `server → store → core`, `server → proto`.

## Request flow

```
client ──REST/gRPC──▶ extractor (ApiKeyAuth | SessionAuth | AdminAuth)
                        │
                        ▼
                   services::*   (business rules, metrics)
                        │
          ┌─────────────┼──────────────┐
          ▼             ▼              ▼
     store::pg     store::cache    store::clickhouse
    (PostgreSQL)      (Redis)       (analytics)
```

### Credentials

| Credential | Carried as | Used for |
|------------|------------|----------|
| client API key `zk_c_…` | `x-api-key` / HTTP Basic username / gRPC metadata | `Authenticate*` |
| server API key `zk_s_…` | same | trusted backends acting for any identity (Nakama module) |
| admin API key `zk_a_…` or bootstrap key | same | `/admin/v1/*` |
| session access token (JWT, HS256) | `Authorization: Bearer` | everything under `/v1` after authenticate |
| refresh token (JWT, HS256, `typ=refresh`) | request body | `AuthenticateRefresh` |

Keys are stored as SHA-256 digests; lookups are cached in Redis for 30 s. The bootstrap key from
configuration is compared in constant time and never written to the database.

### Sessions

* Access tokens are stateless (default TTL 1 h). Revocation is a Redis flag `revoked:<sid>` that lives
  slightly longer than the access TTL, so a logout is effective immediately without a database read.
* Refresh tokens are bound to a `sessions` row holding the SHA-256 of the current refresh token. Refresh
  rotates the token atomically (`UPDATE … WHERE refresh_hash = $old`). A replayed refresh token therefore
  fails the compare-and-swap, and the whole session is revoked — the standard defence against stolen
  refresh tokens.
* Every token carries a random `jti`, so two tokens issued within the same second are still distinct.

### Identities and properties

An identity is keyed by a client-supplied `external_id` (device id, Nakama user id, …) and has three
JSON property namespaces:

* `default` — supplied by the client at authenticate / update time (platform, country, app version);
* `custom` — free-form game data;
* `computed` — derived server-side from analytics events and rules (session count, days since install).

Targeting rules address them as `default.country`, `custom.level`, `computed.sessions`. Merging a patch
follows JSON-merge-patch semantics: `null` deletes a key. Namespaces are capped at 256 keys / 4 KiB per
value so a single misbehaving client cannot bloat the row; the GIN indexes on the `jsonb` columns keep
admin searches cheap.

## Storage

**PostgreSQL** holds everything that is a source of truth: identities, sessions, API keys, and (next)
flag/audience/experiment/event/message definitions plus per-identity assignments. Migrations live in
`migrations/` and are applied on start when `database.migrate = true`.

**Redis** is a cache and a bus, never a source of truth: API-key principals, session revocation flags,
and the `zanshin:events:*` pub/sub channels that replicate configuration changes to all replicas and
to Nakama modules. Losing Redis degrades to slower reads, not to data loss.

**ClickHouse** receives analytics events in batches (`events` table, `MergeTree`, partitioned by month,
400-day TTL). When disabled, events are written to a PostgreSQL table instead so small deployments do
not need a third database.

## Observability

* `tracing` with `RUST_LOG`/`log.level` filters; JSON output with `log.json = true`.
* `metrics` + Prometheus exporter at `/metrics` (`zanshin_*` counters, HTTP error counters by code).
* `x-request-id` is generated when missing and propagated to the response.
* `/healthz` is liveness, `/readyz` pings every configured backend; gRPC exposes
  `grpc.health.v1.Health` and server reflection.

## Nakama bridge (planned)

`bridges/nakama` is a Go runtime module built on `sdk/go`. On `InitModule` it opens a gRPC connection
to Zanshin with a server key, loads the flag / audience / experiment / live-event model, and subscribes
to the change stream. It registers RPCs mirroring the client API (`zanshin.authenticate`,
`zanshin.flags`, `zanshin.experiments`, `zanshin.events`, …) and hooks `AfterAuthenticate*` /
`SessionStart` to keep identities and default properties in sync. Evaluation is local; only writes
(events, property updates) go over the wire.
