# Running Zanshin in production

This is the operator's guide: topology, sizing, deployment (Compose or Helm), upgrades, backups,
disaster recovery, observability and the security checklist. The API itself is in
[api.md](api.md); the design in [ARCHITECTURE.md](ARCHITECTURE.md); releases in
[releasing.md](releasing.md).

## Topology

```
clients / game servers / Nakama
        │  HTTPS + WSS (7550), gRPC (7549)
        ▼
  TLS terminator (Caddy / ingress)  ── /admin, /console restricted by source CIDR; /metrics never public
        │
  N × zanshin (stateless)           ── every replica serves everything; housekeeping jobs are
        │                               serialised cluster-wide through Postgres advisory locks
  ┌─────┼───────────┐
  ▼     ▼           ▼
Postgres  Redis   ClickHouse (optional; events fall back to Postgres)
(truth)   (cache, pub/sub, rate limits, dedupe)   (analytics events)
```

* **Stateless replicas.** Any number of `zanshin` processes behind one load balancer. Flag/audience
  catalogs are replicated through Redis pub/sub, event buffers live in-process and flush every few
  hundred milliseconds. Sticky sessions are not required (WebSocket streams reconnect on their own).
* **PostgreSQL** is the only source of truth. Everything in Redis can be rebuilt; losing ClickHouse
  loses analytics history but no configuration or player state.
* **Redis** is required for: rate limiting (fails open when unreachable), cross-replica cache
  invalidation, event dedupe, QA time travel. A single instance or a managed cluster both work.
* **ClickHouse** is recommended above ~1M events/day. Without it, events go into Postgres
  (`zanshin_events` table) with `retention.events` pruning.

## Sizing

Measured with `deploy/k6/baseline.js` (release build, one replica, rate limits off) on a single
8-vCPU / 31 GiB box that also ran PostgreSQL 16, Redis 7, ClickHouse 24.8 (Docker) **and** the k6
load generator — so absolute numbers are conservative, the shape is what matters:

| Target rate | Achieved | p50 | p95 | p99 | zanshin CPU | RSS |
| --- | --- | --- | --- | --- | --- | --- |
| 500 req/s | 400 req/s* | 2.0 ms | 3.1 ms | 5.7 ms | 0.6 cores | 170 MiB |
| 1 000 req/s | 800 req/s* | 2.3 ms | 3.8 ms | 8.1 ms | 1.2 cores | 170 MiB |
| 2 000 req/s | saturated — see below | | | | | |

\* averages include the 30 s ramp-up, the plateau runs at the target rate.

At 2 000 req/s the shared box runs out of CPU (server ≈ 4 cores + databases + k6). Without a cap
the server kept answering but latency collapsed to p50 ≈ 1.4 s and RSS grew to 1.6 GiB with the
backlog. With `server.http_concurrency` the overflow is answered with a fast `503 + Retry-After`
(measured with a cap of 2 048: 31 % shed, p50 of served requests 0.47 s, RSS ≈ 1 GiB — about
0.5 MiB per in-flight request, which is why the default cap is 1 024). Treat the saturation point
of *your* deployment as the number to find with k6, then run replicas at ≤ 60 % of it and let the
HPA / `ZanshinLoadShedding` alert add capacity before shedding becomes routine.

Rule of thumb: **1 vCPU ≈ 800–1 000 client req/s**, 60–80 MiB RSS per 10 000 open WebSocket
streams, Postgres pool `database.max_connections = 16–32` per replica (Postgres `max_connections`
≥ replicas × pool + 20). Authenticate is 3–5× more expensive than a flag read (Argon2-free, but it
writes); bursts of installs are what to size for.

Starting points:

| Players (DAU) | zanshin | Postgres | Redis | ClickHouse |
| --- | --- | --- | --- | --- |
| < 50 k | 2 × (1 vCPU, 512 MiB) | 2 vCPU, 4 GiB, 50 GiB | 256 MiB | optional, 2 vCPU, 4 GiB |
| 50 k – 500 k | 3 × (2 vCPU, 1 GiB) | 4 vCPU, 16 GiB, 200 GiB + replica | 1 GiB | 4 vCPU, 16 GiB, 500 GiB |
| > 500 k | HPA 4–12 × (2 vCPU, 1 GiB) | managed HA (8+ vCPU) | managed HA | cluster / ClickHouse Cloud |

Measure your own: `k6 run -e RATE=<n> deploy/k6/baseline.js` until a threshold breaks.

## Deploy with Docker Compose (single host)

`deploy/compose/` is a hardened single-host stack: Caddy with automatic TLS (HTTP/1.1, h2, h3),
two `zanshin` replicas, Postgres 16, Redis 7, ClickHouse 24.8, a nightly backup sidecar, and an
optional `observability` profile with Prometheus + Grafana (provisioned dashboard and alerts).

```sh
cd deploy/compose
cp .env.example .env            # set ZANSHIN_DOMAIN, ACME_EMAIL, ADMIN_ALLOW_CIDR, sizing
./init-secrets.sh               # random secrets into ./secrets/* (mode 600); prints the admin key
docker compose pull
docker compose up -d            # migrate → zanshin-1/2 → caddy; readyz gates the proxy
docker compose --profile observability up -d   # + prometheus, grafana at /grafana/
```

What the stack enforces:

* `server.production=true` — the server refuses placeholder secrets, open CORS, anonymous auth.
* Secrets only via files (`ZANSHIN_FILE__…`), containers are `read_only`, `no-new-privileges`,
  all capabilities dropped, Postgres/Redis/ClickHouse live on an `internal` network.
* Caddy: HSTS, least-connections balancing with `/readyz` health checks, WebSocket pass-through,
  `/admin/*` and `/console` answered 403 outside `ADMIN_ALLOW_CIDR`, `/metrics` hidden, gRPC on
  `:7549` with TLS.
* Resource limits and log rotation on every service.

Upgrade: bump `ZANSHIN_IMAGE` in `.env`, then `docker compose pull && docker compose up -d`. The
`migrate` one-shot runs first; replicas are replaced one at a time (`zanshin-1`, then `zanshin-2`)
and Caddy stops routing to a replica as soon as its `/readyz` turns 503 during drain.

## Deploy with Helm (Kubernetes)

`deploy/helm/zanshin` ships the server only — bring PostgreSQL, Redis and ClickHouse from managed
services or operators (CloudNativePG, Redis Operator, Altinity ClickHouse Operator).

```sh
kubectl create namespace liveops
kubectl -n liveops create secret generic zanshin \
  --from-literal=database-url='postgres://zanshin:…@pg.internal:5432/zanshin?sslmode=require' \
  --from-literal=redis-url='redis://:…@redis.internal:6379/0' \
  --from-literal=clickhouse-password='…' \
  --from-literal=jwt-secret="$(openssl rand -base64 48)" \
  --from-literal=admin-bootstrap-key="$(openssl rand -base64 30)"
helm upgrade --install zanshin deploy/helm/zanshin -n liveops \
  --set publicUrl=https://liveops.example.com \
  --set ingress.host=liveops.example.com \
  --set 'ingress.adminAllowCidrs={10.0.0.0/8}' \
  --set secrets.existingSecret=zanshin \
  --set config.clickhouse.url=http://clickhouse.internal:8123 \
  --set metrics.serviceMonitor.enabled=true --set metrics.prometheusRule.enabled=true
```

The chart gives you: rolling updates with `maxUnavailable: 0`, a `pre-upgrade` migration Job,
startup/liveness (`/healthz`) and readiness (`/readyz`) probes, `terminationGracePeriodSeconds`
aligned with `shutdown_grace + shutdown_timeout`, HPA on CPU, PodDisruptionBudget, topology spread,
non-root read-only containers, optional NetworkPolicy, ServiceMonitor and PrometheusRule (the same
rules as `deploy/helm/zanshin/alerts.yml`), a separate allow-listed Ingress for `/admin` and
`/console`, optional gRPC Ingress.

## Configuration

Everything in `config/zanshin.example.yml` can be set with `ZANSHIN__SECTION__KEY` and secrets with
`ZANSHIN_FILE__SECTION__KEY=/path` (Docker/Kubernetes secrets). Lists are TOML/JSON literals
(`ZANSHIN__SERVER__CORS_ORIGINS='["https://a", "https://b"]'`). Production essentials:

| Key | Production value | Why |
| --- | --- | --- |
| `server.production` | `true` | fail fast on unsafe config |
| `server.trust_proxy` / `server.hsts` | `true` behind TLS termination | correct client IPs for rate limits + audit, HSTS header |
| `server.cors_origins` | your console/web origins | browsers only |
| `server.shutdown_grace` / `shutdown_timeout` | `5s` / `30s` | drain window for the LB, then bounded wait for streams |
| `server.http_concurrency` / `grpc_concurrency` | `1024` / `1024` per replica | load shedding: fast `503 + Retry-After` instead of latency collapse when a replica is overloaded; budget ≈ 0.5 MiB of RSS per in-flight request |
| `database.max_connections` | 16–32 per replica | see sizing |
| `database.migrate` | `true` (Compose) / `false` (Helm, Job migrates) | one migrator at a time |
| `auth.access_ttl` / `refresh_ttl` | `1h` / `30d` | refresh replays are detected and revoke the family |
| `clickhouse.retention_days`, `retention.*` | per your data policy | TTL + housekeeping purges |
| `log.json` | `true` | one JSON line per event, `request_id` included |
| `telemetry.otlp_endpoint`, `sample_ratio` | collector URL, `0.05–0.2` | traces without drowning the collector |
| `limits.*` | defaults, tune per key | token buckets; `429` + `Retry-After` |

Check what a container actually resolved: `zanshin --check-config` prints the effective config with
secrets redacted.

## Upgrades

* **Zero-downtime, additive migrations.** Every migration is forward-compatible with the previous
  minor: new columns are nullable or defaulted, nothing is dropped in the same release it stops
  being used. Run order is `zanshin --migrate` (or Compose `migrate` / Helm hook) → roll replicas.
* **Rollback** of a minor: redeploy the previous image; the schema stays (additive). A failed
  migration leaves the transaction rolled back — fix and re-run `--migrate`.
* **Config catalogs** (flags, audiences, experiments…) are replicated through Redis; mixed
  versions during a rollout read the same rows, so a rolling update needs no freeze.
* Skipping minors is supported (migrations are sequential and idempotent); read `CHANGELOG.md`
  for deprecations first.

## Backups and restore

Compose ships the `backup` sidecar (`deploy/compose/backup`): `pg_dump` custom format (zstd) plus
schema, a native ClickHouse `BACKUP DATABASE` into the ClickHouse volume **and** a per-table
native export, `SHA256SUMS`, rotation by `BACKUP_KEEP_DAYS`, optional mirror to any S3-compatible
bucket (`BACKUP_S3_URL`, key + secret file). Default schedule: 03:00 UTC daily.

```sh
docker compose run --rm backup backup.sh                 # ad-hoc backup now
docker compose exec backup ls /backups                   # list
docker compose stop zanshin-1 zanshin-2                  # restore: stop writers
docker compose run --rm backup restore-pg.sh /backups/<ts>
docker compose run --rm backup restore-ch.sh /backups/<ts>
docker compose start zanshin-1 zanshin-2
```

On Kubernetes use the database's own tooling (CloudNativePG scheduled backups / WAL archiving,
ClickHouse `BACKUP … TO S3(...)`). Minimum policy: daily full + 14 days retention + offsite copy;
point-in-time recovery through WAL archiving for Postgres when you can.

Test restores. A backup you have never restored is a hope, not a backup — `restore-pg.sh` into a
scratch database on a schedule is cheap.

## Disaster recovery

| Lost | Impact | Recovery |
| --- | --- | --- |
| a `zanshin` replica | none (LB routes around it) | scheduler / Compose restart |
| Redis | rate limiting fails open, cross-replica invalidation and dedupe pause, QA time travel resets | restart Redis; replicas reconnect, catalogs re-sync from Postgres on next change |
| ClickHouse | event ingest **drops** after the in-memory queue fills (`ZanshinEventsDropped` alert), dashboards empty | restore from backup; ingest resumes automatically |
| Postgres | full outage (`/readyz` 503 everywhere) | restore latest dump (+ WAL replay); RPO = backup interval, RTO ≈ restore time + `--migrate` |
| TLS terminator | outage | redeploy Caddy / ingress; certificates are in `caddy_data` / cert-manager |

Runbook order after a full-host loss: provision host → `init-secrets.sh` with the **same** secret
files (JWT secret and bootstrap key must match or every session/key is invalidated) → restore
Postgres → `docker compose up -d` → restore ClickHouse → verify `/readyz`, console login, a flag
read with a client key.

## Observability

* `GET /metrics` (Prometheus): `zanshin_http_requests_total{method,route,status}`,
  `zanshin_http_request_seconds`, gRPC equivalents, `zanshin_events_*`, `zanshin_db_pool_connections`,
  `zanshin_ws_connections`, `zanshin_job_seconds` / `zanshin_job_last_run_seconds`, LiveOps counters
  (flags, experiments, messages, push, webhooks, journeys, exports). Never expose it publicly.
* Dashboard: `deploy/observability/grafana/dashboards/zanshin-overview.json` (overview, HTTP, gRPC,
  events, LiveOps, identities, dependencies).
* Alerts: `deploy/helm/zanshin/alerts.yml` — instance down, 5xx / gRPC error ratio, client API p99,
  events dropped / queue backlog / slow flush, pool saturation, rate limiter degraded,
  housekeeping stalled, push / webhook / export failures, refresh-token replays.
* Traces: set `telemetry.otlp_endpoint` (OTLP/gRPC) — every HTTP/gRPC request is a span with
  `request_id`, DB and Redis calls as children; `sample_ratio` for head sampling.
* Logs: JSON lines on stdout; `request_id` correlates with the `x-request-id` response header and
  audit log entries.
* Readiness: `GET /readyz` returns the status of Postgres, Redis, ClickHouse and the pool; `503`
  during drain. Liveness: `GET /healthz`.

## Security checklist

- [ ] `server.production=true`; `zanshin --check-config` shows no `change-me` anywhere.
- [ ] TLS everywhere: terminator in front (HSTS on), `sslmode=require` to managed Postgres, Redis
      with AUTH (+ TLS off-host), ClickHouse password set.
- [ ] `/admin/*`, `/console`, `/metrics` reachable only from trusted networks (Caddyfile
      `ADMIN_ALLOW_CIDR`, Helm `ingress.adminAllowCidrs`).
- [ ] Bootstrap admin key used once to mint personal admin keys (and SSO via `admin.oidc`), then
      rotated; server keys for game servers / Nakama only, client keys in the game builds.
- [ ] Rate limits on (`limits.enabled`), per-key quotas for anything public.
- [ ] Audit log retention set and shipped (`GET /admin/v1/audit`, `audit.retention`).
- [ ] Backups verified by an actual restore; secrets backed up separately from data.
- [ ] Images pinned by digest or version, signature verified (`cosign verify`, see releasing.md);
      `security.yml` green on the release commit.
- [ ] Containers non-root, read-only filesystem, capabilities dropped (default in both kits).
- [ ] Webhook / integration secrets rotated when staff change; `SECURITY.md` contact up to date.

## Capacity and baselines

Re-run after every infra change and keep the numbers in your own runbook:

```sh
k6 run deploy/k6/smoke.js                        # after each deploy — strict, 20 iterations
k6 run -e RATE=1000 -e DURATION=5m deploy/k6/baseline.js
k6 run -e STREAMS=2000 deploy/k6/stream.js       # WebSocket fan-out
```

Thresholds in the scripts mirror the service level this document assumes: reads and ingest
p95 < 50 ms / p99 < 150 ms, authenticate p95 < 150 ms, error rate < 0.1 %.

## `zanshin --doctor`

Run it on the host (same env / config file as the server) before serving traffic and after every upgrade:

```
zanshin --doctor          # human-readable, exit 1 when anything FAILs
zanshin --doctor --json   # for scripts / monitoring
```

Checks: `auth.jwt_secret` strength, bootstrap key hygiene, production/proxy flags, console SSO (OIDC / SAML / SCIM, SAML certificates parse), push provider, PostgreSQL + pool, Redis, ClickHouse (when enabled), pending migrations, clock skew between host and database, configured integrations, active console users.

## What CI verifies against real services

The `live` job runs on every push: Keycloak (OIDC + SAML logins incl. role mapping and rejection without a mapped group), ClickHouse (SQL explorer, path analysis, metric alerts), Mailpit (SMTP delivery), RustFS (S3-compatible export, JSONL and Parquet), Nakama 3.x with the Zanshin module (RPCs, zero-code identity sync), plus Okta / Entra SCIM request shapes. For S3-compatible self-hosted storage we use and test **RustFS**.
