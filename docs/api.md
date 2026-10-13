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

### `GET /v1/healthcheck`

→ `{}`; no credentials required.

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

## Operations

| Path | Purpose |
|------|---------|
| `GET /` | name, version, start time |
| `GET /healthz` | liveness |
| `GET /readyz` | readiness — pings PostgreSQL, Redis and ClickHouse (if enabled) |
| `GET /metrics` | Prometheus text format |

gRPC: `grpc.health.v1.Health/Check`, `grpc.reflection.v1.ServerReflection`.
