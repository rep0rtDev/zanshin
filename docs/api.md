# API reference

Base URL: `http://host:7550`. All bodies are JSON. Errors are
`{"code": "<invalid_argument|unauthenticated|permission_denied|not_found|conflict|unavailable|internal>", "message": "…"}`
with the matching HTTP status (400 / 401 / 403 / 404 / 409 / 503 / 500).

Credentials:

* **API key** — `x-api-key: <key>` or `Authorization: Basic base64("<key>:")`.
* **Session** — `Authorization: Bearer <access token>`.

The gRPC service `zanshin.api.v1.Zanshin` (port 7549) exposes the same operations with the same
metadata headers; see `proto/zanshin/api/v1/zanshin.proto`.

## Client API — `/v1`

### `POST /v1/authenticate` — client key

```json
{ "id": "player-42", "default": { "platform": "android" }, "custom": { "level": 3 } }
```

Creates the identity on first use, merges the supplied properties, returns:

```json
{
  "token": "<access jwt>",
  "refresh_token": "<refresh jwt>",
  "identity": { "id": "uuid", "external_id": "player-42", "created_at": "…", "updated_at": "…", "last_seen_at": "…" },
  "properties": { "default": {…}, "computed": {…}, "custom": {…} }
}
```

`id`: 1–128 bytes, no whitespace / control characters. Property keys match
`^[A-Za-z_][A-Za-z0-9_.-]{0,127}$`; at most 256 keys per namespace, 4 KiB per value.

### `POST /v1/authenticate/refresh` — client key

`{ "refresh_token": "…" }` → same body as authenticate. The refresh token is rotated; reusing a consumed
refresh token revokes the whole session (401 `refresh token already used`).

### `POST /v1/authenticate/logout` — client key

`{ "token": "…", "refresh_token": "…" }` (either is enough) → `{}`. Access tokens of the session are
rejected immediately afterwards.

### `GET /v1/identity` — session

Returns the identity object (without properties).

### `DELETE /v1/identity` — session

Deletes the identity, its sessions and attached data. → `{}`

### `GET /v1/properties` — session

→ `{ "default": {…}, "computed": {…}, "custom": {…} }`

### `PUT /v1/properties` — session

```json
{ "default": { "country": null }, "custom": { "level": 4, "vip": true }, "recompute": false }
```

JSON-merge-patch: `null` removes a key. `computed` cannot be written by clients. Returns the full
property set.

### `GET /v1/flags?names=a,b` — session

Evaluates feature flags for the current identity (`names` omitted = every flag):

```json
{ "flags": [
  { "key": "shop_v2", "value": true, "value_type": "bool", "reason": { "kind": "rule", "rule_id": "de" }, "version": 3 },
  { "key": "ab_banner", "value": "red", "value_type": "string", "variant": "a",
    "reason": { "kind": "variant", "rule_id": "ab", "variant": "a" }, "version": 1 }
] }
```

`reason.kind` is one of `disabled`, `default`, `override`, `experiment`, `rule`, `variant` —
listed in precedence order.

### `GET /v1/flags/{key}` — session

One evaluation; 404 when the flag does not exist.

### `GET /v1/audiences` — session

`{ "audiences": ["beta_testers", "whales"] }` — keys of every enabled audience whose condition
(and sampling percentage) the identity satisfies. Evaluated on read; nothing is stored.

### `GET /v1/experiments?names=a,b` — session

Enrolments of the current identity. On first call for a running experiment the identity is
targeted, allocated and the variant is stored, so later calls return the same variant even if
properties change:

```json
{ "experiments": [ { "key": "price_test", "name": "Price test", "variant": "cheap", "value": 4.99, "version": 2 } ] }
```

Paused experiments keep serving existing assignments without enrolling anyone new; stopped and
draft experiments return nothing.

### `POST /v1/experiments/{key}/expose` — session

Records that the identity actually saw its variant (`exposures` + 1, first `exposed_at`). 404 when
not enrolled, 400 when the experiment is not live.

### `GET /v1/live-events?names=a,b&upcoming=true` — session

Live events whose targeting (rules + audiences) matches the identity. By default only events with
a current occurrence are returned; `upcoming=true` adds targeted events that are not active yet,
each with its `next` occurrence.

```json
{ "live_events": [ {
  "key": "weekend_xp", "name": "Double XP weekend", "description": "", "value": { "xp": 2 },
  "active": true,
  "current": { "start": "2026-01-03T18:00:00Z", "end": "2026-01-05T18:00:00Z" },
  "next":    { "start": "2026-01-10T18:00:00Z", "end": "2026-01-12T18:00:00Z" },
  "version": 3
} ] }
```

### `GET /v1/messages?limit=50` — session

Inbox. Every live campaign that targets the identity and has not been delivered yet is delivered
first (one copy per identity, title/body rendered), then unconsumed, unexpired copies are returned
ordered by priority then recency.

```json
{ "messages": [ {
  "id": "01a1…", "key": "welcome_gift", "title": "Welcome, Neo", "body": "Here is your gift.",
  "image_url": "", "payload": { "gift": 100 }, "priority": 5,
  "delivered_at": "…", "read_at": null, "expires_at": "…"
} ] }
```

`key` is `null` for direct messages.

### `PUT /v1/messages/{id}` · `DELETE /v1/messages/{id}` — session

`PUT` marks the copy read (`{ "read": true }`) and returns it. `DELETE` consumes it: it disappears
from the inbox and is never redelivered.

### `GET /v1/stream?token=<access token>&names=a,b` — WebSocket

Live changes for the current identity. `Authorization: Bearer` also works where headers can be
set. Messages from the server:

```json
{ "type": "hello", "identity_id": "…" }
{ "type": "flag", "flag": { …evaluation… } }        // definition or override changed
{ "type": "flag_deleted", "key": "shop_v2" }
{ "type": "flags", "flags": [ … ] }                  // after an audience / experiment change
{ "type": "experiments", "experiments": [ … ] }
{ "type": "live_events", "live_events": [ … ] }      // after a live event change (active + upcoming)
{ "type": "inbox", "messages": [ … ] }               // after a campaign change or a direct message
{ "type": "error", "message": "session expired" }   // followed by close
```

Send `{ "type": "ping" }` to receive `{ "type": "pong" }`. gRPC clients use `StreamFlags`.

### `GET /v1/healthcheck`

→ `{}`; no credentials required.

## Server API — `/server/v1` (server or admin key)

For trusted backends (game servers, the Nakama module). Identities are addressed by external id.

| Method | Path | Returns |
|--------|------|---------|
| `GET` | `/flags?external_id=<id>&names=a,b` | `{ "flags": [ …evaluations… ] }` |
| `GET` | `/audiences?external_id=<id>` | `{ "audiences": [ … ] }` |
| `GET` | `/experiments?external_id=<id>&names=a,b` | `{ "experiments": [ … ] }` (assigns when eligible) |
| `POST` | `/experiments/{key}/expose?external_id=<id>` | `{}` |
| `GET` | `/live-events?external_id=<id>&names=a,b&upcoming=true` | `{ "live_events": [ … ] }` |
| `POST` | `/messages` body `{ "external_id", "title", "body", "image_url", "payload", "priority", "ttl_secs" }` | `201 { "id": "…" }` — direct message into that identity's inbox |

gRPC: `ServerGetFlags`, `ServerGetAudiences`, `ServerGetExperiments`, `ServerGetLiveEvents`, `ServerSendMessage`.

## Admin API — `/admin/v1` (admin key)

| Method | Path | Body / query | Returns |
|--------|------|--------------|---------|
| `POST` | `/api-keys` | `{ "name": "…", "kind": "client|server|admin" }` | key row + `secret` (shown once) |
| `GET` | `/api-keys` | — | `[ { id, name, kind, prefix, created_at, last_used_at, revoked_at } ]` |
| `DELETE` | `/api-keys/{id}` | — | revoked key row |
| `GET` | `/identities` | `?after=<uuid>&limit=100` (max 500) | `{ "items": [ identity + properties ], "next": uuid|null }` |
| `GET` | `/identities/{id}` | — | identity + properties |
| `GET` | `/identities/by-external-id/{external_id}` | — | identity + properties |
| `PUT` | `/identities/{id}/properties` | `{ "default": {…}, "custom": {…} }` | identity + properties |
| `DELETE` | `/identities/{id}` | — | `{}` |
| `GET` | `/stats` | — | `{ "identities": n, "uptime_seconds": n, "clickhouse_enabled": bool }` |
| `GET` | `/flags` | — | every flag definition |
| `POST` | `/flags` | flag definition (below) | `201` + definition |
| `GET` / `PUT` / `PATCH` / `DELETE` | `/flags/{key}` | partial definition for `PUT`/`PATCH` | definition (`version` increments on every write) |
| `GET` | `/flags/{key}/overrides?limit=` | — | `[ { identity_id, external_id, value } ]` |
| `PUT` / `DELETE` | `/flags/{key}/overrides/{identity_id}` | `{ "value": … }` | `{}` |
| `GET` | `/flags/{key}/evaluate/{identity_id}` | — | evaluation for that identity (debug) |
| `POST` | `/flags/{key}/evaluate` | `{ "subject": "…", "properties": { "default": {…}, "custom": {…} } }` | dry-run evaluation with ad-hoc properties |

| `GET` / `POST` | `/audiences` | `{ "key", "name", "description", "condition", "percentage", "enabled", "salt", "tags" }` | audience definitions |
| `GET` / `PUT` / `PATCH` / `DELETE` | `/audiences/{key}` | partial definition | definition |
| `GET` | `/audiences/{key}/evaluate/{identity_id}` | — | `{ "member": bool }` |
| `POST` | `/audiences/{key}/evaluate` | `{ "subject", "properties" }` | dry-run membership |
| `GET` / `POST` | `/experiments` | experiment definition (below) | definitions |
| `GET` / `PUT` / `PATCH` / `DELETE` | `/experiments/{key}` | partial definition | definition |
| `POST` | `/experiments/{key}/start` · `/pause` · `/stop` | — | definition; transitions: draft→running, running⇄paused, running/paused→stopped |
| `POST` | `/experiments/{key}/reset` | — | `{ "deleted_assignments": n }` |
| `GET` | `/experiments/{key}/results` | — | assigned / exposed / exposures per variant |
| `GET` / `PUT` / `DELETE` | `/experiments/{key}/assignments/{identity_id}` | `{ "variant": "…" }` for `PUT` | inspect / force / clear one identity's assignment |

| `GET` / `POST` | `/live-events` | live event definition (below) | definitions |
| `GET` / `PUT` / `PATCH` / `DELETE` | `/live-events/{key}` | partial definition | definition |
| `GET` / `POST` | `/messages` | message campaign (below) | definitions |
| `GET` / `PUT` / `PATCH` / `DELETE` | `/messages/{key}` | partial definition | definition |
| `GET` | `/messages/{key}/stats` | — | `{ "delivered", "read", "consumed" }` |
| `POST` | `/identities/{identity_id}/messages` | direct message (same body as the server API minus `external_id`) | `201 { "id" }` |

### Live event definition

```json
{
  "key": "weekend_xp", "name": "Double XP weekend", "enabled": true,
  "condition": { "prop": "default.country", "op": "neq", "value": "CN" },
  "audiences": ["active_players"],
  "starts_at": "2026-01-01T00:00:00Z",   // first occurrence / schedule start
  "ends_at": null,                        // hard end of the whole event (optional)
  "recurrence": "0 0 18 * * Sat *",       // cron, UTC: sec min hour day month weekday [year]; omit for one-shot
  "duration_secs": 172800,                // length of each occurrence (required when recurring)
  "payload": { "xp": 2 }, "tags": []
}
```

A one-shot event needs `ends_at` or `duration_secs`.

### Message campaign definition

```json
{
  "key": "welcome_gift", "name": "Welcome gift", "enabled": true,
  "title": "Welcome, {{custom.nickname}}", "body": "Here is your gift.", "image_url": "", "payload": { "gift": 100 },
  "condition": { "prop": "computed.session_count", "op": "lte", "value": 1 },
  "audiences": [], "percentage": 100,
  "starts_at": null, "ends_at": null,     // campaign window
  "ttl_secs": 604800,                     // how long a delivered copy stays in the inbox
  "priority": 5, "tags": []
}
```

`{{path}}` placeholders in `title` and `body` are rendered from the identity's flattened properties
at delivery time.

### Experiment definition

```json
{
  "key": "price_test", "name": "Price test", "hypothesis": "A lower entry price lifts conversion",
  "status": "draft",                       // draft | running | paused | stopped (use the lifecycle endpoints)
  "condition": { "prop": "default.country", "op": "in", "value": ["DE", "AT"] },
  "audiences": ["new_players"],            // all must match
  "percentage": 50,                        // share of eligible identities enrolled
  "variants": [ { "key": "control", "value": 9.99, "weight": 50 }, { "key": "cheap", "value": 4.99, "weight": 50 } ],
  "control": "control",
  "flag_key": "price",                     // optional: the assigned variant's value becomes this flag's value
  "salt": "", "starts_at": null, "ends_at": null, "tags": []
}
```

When `flag_key` is set, flag evaluation reports `"reason": { "kind": "experiment", "experiment": "price_test", "variant": "cheap" }`
and the variant key, so clients can attribute metrics without a second lookup. Per-identity flag
overrides still win over experiments.

### Flag definition

```json
{
  "key": "shop_v2",
  "name": "New shop", "description": "…", "tags": ["shop"],
  "value_type": "bool",            // bool | string | number | json
  "default_value": false,          // served when disabled or when no rule matches
  "enabled": true,
  "salt": "",                      // change to reshuffle every rollout bucket of this flag
  "rules": [
    {
      "id": "de-rollout", "name": "Germany, 50 %",
      "condition": { "all": [
        { "prop": "default.country", "op": "in", "value": ["DE", "AT"] },
        { "prop": "default.app_version", "op": "semver_gte", "value": "1.4.0" },
        { "not": { "prop": "custom.banned", "op": "exists" } }
      ] },
      "audiences": [],               // audience keys (all must match) — lands with audiences
      "percentage": 50,              // 0..100, fractional allowed; deterministic per identity
      "value": true                  // or "variants": [ { "key": "a", "value": …, "weight": 70 }, … ]
    }
  ]
}
```

Rules are evaluated in order; the first whose condition matches *and* whose rollout bucket includes
the identity wins. Bucketing hashes `flag key + salt + rule id + external id`, so a rollout can be
raised from 10 % to 20 % without moving anyone who was already in.

Condition operators: `eq` `neq` `gt` `gte` `lt` `lte` `in` `not_in` `contains` `not_contains`
`starts_with` `ends_with` `matches` (regex) `exists` `not_exists` `semver_eq` `semver_gt`
`semver_gte` `semver_lt` `semver_lte`. Properties are addressed as `default.*`, `custom.*`,
`computed.*`, plus `identity.id` and `identity.external_id`. Comparisons tolerate `"5"` vs `5`
and `"true"` vs `true`.

## Operations

| Path | Purpose |
|------|---------|
| `GET /` | name, version, start time |
| `GET /healthz` | liveness |
| `GET /readyz` | readiness — pings PostgreSQL, Redis and ClickHouse (if enabled) |
| `GET /metrics` | Prometheus text format |

gRPC: `grpc.health.v1.Health/Check`, `grpc.reflection.v1.ServerReflection`.
