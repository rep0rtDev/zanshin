# API reference

Machine-readable version: `docs/openapi.yaml` (served at `/openapi.yaml` and `/openapi.json`) with Swagger UI at `/docs` — client and server APIs are fully typed, admin endpoints are listed with their permission model.

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

### `POST /v1/identify` — session

Link the current identity to a new external id (anonymous → account). If no identity owns `id` the
external id is renamed in place; otherwise the current identity is **merged** into the existing one:
custom / default properties move (the target wins on conflict), computed counters are summed, events,
experiment assignments, flag overrides, inbox and push devices are re-attached, the merge is recorded
(`GET /admin/v1/identities/{id}/merges`) and the source identity is deleted. The old session is closed;
the response is a fresh session for the resulting identity (same shape as `POST /v1/authenticate`).

```json
{ "id": "steam:7656119" }
```

Server-side equivalent (server key): `POST /server/v1/identities/{external_id}/identify` with the same body.

#### Default-property enrichment

On authenticate the server fills `default` properties the client did not send from request headers:
`X-Zanshin-Platform`, `X-Zanshin-OS`, `X-Zanshin-OS-Version`, `X-Zanshin-Device`, `X-Zanshin-App-Version`,
`X-Zanshin-Timezone`, `Accept-Language` (→ `locale`, `language`), and — when `taxonomy.geoip_db` points to a
MaxMind City/Country database — `country`, `region`, `city`, `timezone` from the client IP
(`CF-IPCountry` / `X-Country` are used as a country fallback). Properties sent explicitly always win.

#### Play sessions & synthetic events

Every authenticate / identify opens a play session (`GET /admin/v1/identities/{id}/sessions`); events and
refreshes keep it alive, logout or `taxonomy.session_inactivity` (default 30 min) close it with a duration.
The server emits `$identity_create`, `$session_start`, `$session_end` (value = seconds), `$identify`
(`metadata.from` / `to` / `merged`) and `$experiment_join` (`metadata.experiment`, value = variant).
These never count towards `event_count`; `$session_end` maintains `play_time_sec`.

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

### `GET /v1/flags/overrides` — session

Every override currently applied to the caller, in evaluation order: `identity` (QA / support
override), `experiment` (`source_key` = experiment, `variant`) and `live_event` (`source_key` =
event). `{"overrides": [{"flag_key", "value", "source", "source_key", "variant"}]}`.

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

### `POST /v1/events` — session

Report analytics events. Up to 500 per request; names are `[A-Za-z0-9_.:-]` up to 128 chars and
may not start with `$` (reserved for server-emitted events). `timestamp` more than 30 days old or
5 minutes in the future is replaced by receipt time. `id` is an optional idempotency key.

```json
{ "events": [
  { "name": "purchase", "value": "9.99", "metadata": { "sku": "gold_pack" }, "timestamp": "2026-01-03T18:00:00Z", "id": "0193…" },
  { "name": "level.up", "value": "7" }
] }
→ { "accepted": 2 }
```

Events are queued and written in batches (≈ every 500 ms) to ClickHouse, or to PostgreSQL when
ClickHouse is disabled. Every event is tagged with the identity's current experiment assignments so
results can be sliced per variant without joins. The server itself emits `$authenticate` on every
authenticate and `$exposure` (value = variant, `metadata.experiment`) on exposure.

#### Computed properties

Each flush folds the batch into `computed.*` of the identity, which any rule can target:

| key | meaning |
|---|---|
| `session_count`, `first_seen_at`, `last_seen_at` | from `$authenticate` |
| `event_count`, `first_event_at`, `last_event_at` | all non-`$authenticate` events |
| `events.<name>.count`, `events.<name>.last_at` | per event name (`.` in names becomes `_`) |
| `events.<name>.sum` | when `value` parses as a number |

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
| `POST` | `/events` body `{ "external_id", "events": [ … ] }` | `{ "accepted": n }` — `$`-prefixed names allowed |
| `POST` | `/identities` body `{ "external_id", "default", "custom" }` | identity + properties; creates or merges, no session (emits `$authenticate`) |
| `PUT` | `/identities/{external_id}/properties` body `{ "default", "custom" }` | merged properties (`null` removes) |
| `GET` | `/messages/inbox?external_id=<id>&limit=` | `{ "messages": [ … ] }` (delivers pending campaigns first) |
| `PUT` | `/messages/inbox/{id}?external_id=<id>` | the inbox copy, marked read |
| `POST` | `/messages/inbox/{id}/consume?external_id=<id>` | `{}` — consumed, never redelivered |
| `DELETE` | `/messages/inbox/{id}?external_id=<id>` | `{ "deleted": id }` — removed from the inbox entirely |
| `GET` | `/flags/{key}?external_id=<id>` | a single evaluation |
| `GET` | `/flags/{key}/explain?external_id=<id>` | the override chain for that identity (same shape as the admin explain endpoint) |
| `POST` | `/live-events/{key}/join?external_id=<id>` | the live event view after an explicit join (emits `$live_event_join`) |
| `POST` | `/live-events/{key}/leave?external_id=<id>` | `{ "left": bool }` |
| `POST` | `/identities/{external_id}/identify` body `{ "id" }` | identity + properties after renaming / merging into `id` |
| `POST` / `DELETE` | `/audiences/{key}/members` body `{ "external_ids": [ … ] }` | `{}` — managed-audience membership |
| `WS` | `/stream` (`x-api-key` header or `?api_key=`) | change feed: `{"type":"change","kind":"flag\|audience\|experiment\|live_event\|message","key","version","deleted","identity_id"}`; `{"type":"resync"}` after lag; `ping`/`pong` |

gRPC: `ServerGetFlags`, `ServerGetAudiences`, `ServerGetExperiments`, `ServerGetLiveEvents`, `ServerSendMessage`, `ServerSendEvents`.

## Projects (multi-tenant deployments)

One deployment can host several isolated projects (games or environments). Each non-default project has
its own PostgreSQL schema, Redis namespace and ClickHouse database — configuration, players, events, API
keys and analytics never cross projects; console users and SSO are shared. Pick the project with the
`x-zanshin-project: <slug>` header (or `?project=<slug>`); session tokens issued inside a project carry a
`prj` claim, so client calls with a bearer token or `?token=` need no header. Unknown slugs → 404.
gRPC uses the same selection: the `x-zanshin-project` metadata key, else the `prj` claim of the
`authorization` bearer token (unknown slugs → `NOT_FOUND`). Management lives on the default project and needs `settings:*`:

| Method | Path | Notes |
| --- | --- | --- |
| GET | `/admin/v1/projects` | `{items: [{slug, name, description, archived, running, …}], current}` |
| POST | `/admin/v1/projects` | `{slug, name?, description?}`; slug `[a-z][a-z0-9_]{1,31}`; creates + migrates the schema and starts the project |
| GET | `/admin/v1/projects/{slug}` | |
| PUT | `/admin/v1/projects/{slug}` | `{name?, description?, archived?}`; archiving stops routing but keeps the data, un-archiving starts it again |

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
| `GET` | `/experiments/{key}/results?metric=purchase&from=&to=` | — | assigned / exposed / exposures per variant; with `metric`: `count`, `identities`, `sum`, `conversion` (= identities / assigned) of that event per variant |
| `GET` / `PUT` / `DELETE` | `/experiments/{key}/assignments/{identity_id}` | `{ "variant": "…" }` for `PUT` | inspect / force / clear one identity's assignment |

| `GET` / `POST` | `/live-events` | live event definition (below) | definitions |
| `GET` / `PUT` / `PATCH` / `DELETE` | `/live-events/{key}` | partial definition | definition |
| `GET` / `POST` | `/messages` | message campaign (below) | definitions |
| `GET` / `PUT` / `PATCH` / `DELETE` | `/messages/{key}` | partial definition | definition |
| `GET` | `/messages/{key}/stats` | — | `{ "delivered", "read", "consumed" }` |
| `POST` | `/identities/{identity_id}/messages` | direct message (same body as the server API minus `external_id`) | `201 { "id" }` |

| `GET` | `/analytics/events?name=purchase&from=&to=&granularity=hour\|day` | — | `{ "buckets": [ { "bucket", "count", "identities", "sum" } ], "backend": "clickhouse" \| "postgres" }` (default window: last 7 days) |
| `GET` | `/analytics/events/names?from=&to=&limit=` | — | event names by volume with unique identities |
| `GET` | `/identities/{identity_id}/events?limit=` | — | most recent events of one identity (with `experiments` tags) |

### LiveOps polish

- `GET /admin/v1/flags/{key}/sizes` — serialized byte size of every value a client can receive
  (`default`, `rule:<id>`, `variant:<key>`, `rule:<id>:variant:<key>`).
- `GET /admin/v1/flags/{key}/diff?a=default&b=rule:vip` — leaf-level differences between two
  values (`changes: [{path, from?, to?}]`, `/`-separated paths; an absent side is omitted) plus
  both payload sizes. Unknown selectors are `404`.
- Experiments carry `priority` (default `0`). When several running experiments pin the same flag
  the highest priority wins (ties: earliest `starts_at`); the others still enrol and track metrics.
- `POST /admin/v1/identities/{id}/messages` / server `send_message`: `inbox: false` skips the inbox
  (requires `push` or `channels`), `template_key` or `template_name` renders a stored template
  (locale from `locale`, else `default.locale`), `channels: ["onesignal"]` delivers through
  integrations; inline `title` / `body` work without any template.
- Integrations expose `health` (`last_status`, `last_error`, `last_at`, `sent_count`,
  `failed_count`), updated on every delivery attempt.
- `GET /admin/v1/identities/{id}/events?name=&q=&from=&to=&limit=` — player event search by exact
  name, case-insensitive text in value / metadata and time window.
- `GET /admin/v1/config/export?kinds=flags,templates&keys=a,b` and `keys` on
  `POST /admin/v1/config/import` narrow bundles to specific entities.
- Deleting a flag, audience, template, integration, metric, experiment or live event that another
  entity still references returns `409 Conflict` listing the referrers (`experiment:pricing_v2`).
- Events rejected by the taxonomy are exported to data sinks as `$event_error` (metadata: `event`,
  `reason`, truncated `value`); `POST /v1/events` responds `{"accepted", "rejected"}`.
  GDPR deletion emits `$identity_delete`, which survives the deletion as a tombstone.

### Analytics v2

| Route | Purpose |
|-------|---------|
| `GET /admin/v1/analytics/overview?from&to` | Game metrics: `totals` (dau, dau_avg, wau, mau, total_players, new_users, sessions, avg_session_secs, revenue, purchases, paying_player_days, arpdau, arppu, paying_share, stickiness) and a per-day `series`. Revenue sums `metadata.amount` of events whose taxonomy definition is typed `revenue` plus `purchase` / `revenue` / `iap`. |
| `GET /admin/v1/analytics/retention?from&to&days=1,3,7,14,30` | Cohorts by sign-up day with `retained.dN = { count, rate }` (`null` while the day is in the future). |
| `POST /admin/v1/analytics/funnel` `{ steps: [{ name, field?, op?, value? }], window_secs?, from?, to? }` | Ordered conversion per player within the window; step filters on metadata (`eq`, `neq`, `exists`, `gt/gte/lt/lte`, `contains`). Returns `entered`, per-step `count` / `conversion` / `step_conversion` / `drop`. |
| `GET/POST /admin/v1/analytics/annotations`, `DELETE …/{id}` | Chart markers. Configuration changes (flags, audiences, experiments, live events, messages) are recorded automatically; `POST { title, body?, at? }` adds an operator note. |
| `GET/POST /admin/v1/dashboards`, `GET/PUT/DELETE /admin/v1/dashboards/{key}` | Custom dashboards: `widgets: [{ id, kind, title, width 1–4, config }]` with kinds `timeseries { event }`, `number { metric \| event_count + event }`, `funnel { steps, window_secs }`, `retention { days }`, `top_events { limit }`. |
| `GET /admin/v1/dashboards/{key}/data?from&to` | Evaluates every widget for the window. |

### Journeys

Per-player automations under `/admin/v1/journeys` and the **Journeys** console page.

| Route | Purpose |
|-------|---------|
| `GET/POST /admin/v1/journeys`, `GET/PUT/DELETE /admin/v1/journeys/{key}` | Journey definition: `trigger`, entry `condition` / `audiences`, `steps`, `cooldown_secs` (rejoin cooldown), `max_entries`, `starts_at` / `ends_at`. Responses carry a derived `status`: `disabled`, `scheduled` (before `starts_at`), `running`, `terminated` (after `ends_at`; players already inside keep going). `EnterJourney` steps across journeys must not form a cycle (`400`). |
| `POST /admin/v1/journeys/{key}/enter` `{ identity_id \| external_id }` | Enter a player now (honours targeting, cooldown and caps; ignores the live window). |
| `GET /admin/v1/journeys/{key}/runs?status&after&limit` | Runs with per-status `stats`, per-step `steps[] { active, passed }` (players waiting at / that went through each step) and `timing { completed, min_secs, avg_secs, max_secs }` for finished runs. |
| `GET /admin/v1/journeys/{key}/runs/{id}` | One run with its step log. |
| `POST /admin/v1/journeys/{key}/runs/{id}/exit` | Stop an active run. |
| `GET /admin/v1/identities/{id}/journeys` | A player's runs across journeys, each with its step `log` (what happened, when). |
| `GET /admin/v1/identities/{id}/flags` | Every flag as this player resolves it right now — value, `variant` and `reason` (`default` / `disabled` / `rule` / `variant` / `override` / `experiment` / `live_event`), the console's **Flags** tab on an identity. |

Triggers: `{ "kind": "manual" }`, `{ "kind": "event", "name": "purchase" }`, `{ "kind": "identity_created" }`,
`{ "kind": "audience_enter", "audience": "whales" }` (checked whenever the player produces an event, once per membership),
`{ "kind": "live_event_start", "live_event": "weekend_rush" }` (every targeted player, once per occurrence).

Steps are an ordered list of `{ "id", "kind", … }`:

| kind | fields |
|------|--------|
| `message` | `title`, `body`, `image_url`, `payload`, `template_key`, `push` — inbox message, `{{prop}}` rendered |
| `track` | `name`, `value`, `metadata` — analytics event tagged with `journey` / `journey_run` |
| `flag_override` | `flag`, `value` |
| `experiment_assign` | `experiment`, `variant` |
| `integration` | `integration`, `title`, `body`, `payload` — send through a delivery integration |
| `wait` | `secs` |
| `wait_until` | `hour`, `minute`, `timezone_prop` (default `default.timezone`) — next local occurrence |
| `time_window` | `at` (instant) **or** `cron` (`sec min hour dom month dow [year]`) + `timezone` (`null` = UTC, an IANA zone, or `"player"` to read `timezone_prop`) — hold until a date or the next slot of a schedule |
| `live_event` | `live_event`, `duration_secs` — the event starts for this player now and stays active for the duration (default: the event's own duration, else until its end), whatever its schedule; `GET /v1/live-events` reports it with a personal `current` window |
| `revert` | `step` — undo what that earlier `flag_override` / `experiment_assign` / `live_event` step granted |
| `enter_journey` | `journey` — enrol the player into another journey (its targeting / cooldown apply; the child run's `context.parent` names this journey) |
| `branch` | `condition`, `then`, `else` (step ids; `else` omitted = next step) |
| `goto` | `step` |
| `exit` | — |

Runs are persisted (`journey_runs`), waits wake through the worker every 5s (claimed with `FOR UPDATE SKIP LOCKED`, so
multiple replicas are safe), each step is logged, and `journey.entered` / `journey.finished` are emitted to webhooks.

### Webhooks

Outbound HTTP callbacks for system events, managed under `/admin/v1/webhooks` and the **Webhooks** console page.

| Route | Purpose |
|-------|---------|
| `GET/POST /admin/v1/webhooks`, `GET/PUT/DELETE /admin/v1/webhooks/{key}` | Subscriptions: `url`, `secret` (masked on read), `events` patterns (`*`, `flag.*`, `event.purchase`), `batch_size` / `batch_window_secs`, `headers`, `max_attempts`, `enabled`. |
| `GET /admin/v1/webhooks/event-types` | Known event types. |
| `POST /admin/v1/webhooks/{key}/test` | Send a `webhook.test` event synchronously; returns the delivery (status, attempts, HTTP status, error). |
| `GET /admin/v1/webhooks/{key}/deliveries?status&after&limit` | Delivery log (`pending` / `sent` / `failed` / `dead`) with per-status stats. |
| `POST /admin/v1/webhooks/{key}/deliveries/{id}/retry` | Retry a failed or dead delivery now (fresh attempt budget). |

Event types: `identity.created` / `identity.authenticated` / `identity.identified`, `session.started` / `session.ended`,
`experiment.joined` / `experiment.exposed`, `live_event.joined`, `message.delivered` / `message.read`, `event.<name>` for every
analytics event, and `<flag|audience|experiment|live_event|message>.<updated|deleted>` for configuration changes, `flag.rule.created` / `flag.rule.updated` / `flag.rule.deleted` (one per targeting rule that appeared, changed or vanished in a flag update — Satori's variant events), `journey.step_reached` (a player reached a *Send to integration* step; carries `journey`, `step`, `integration`, delivery `status`), `message.direct_sent` (a direct message was sent from the console or API).

Request: `POST url` with JSON `{ "webhook", "delivery_id", "attempt", "sent_at", "events": [ { "id", "type", "at", "data" } ] }` and
headers `X-Zanshin-Event` (the type, or `batch`), `X-Zanshin-Delivery`, `X-Zanshin-Webhook`, `X-Zanshin-Timestamp`,
`X-Zanshin-Signature: sha256=HMAC_SHA256(secret, "{timestamp}.{body}")`. Any 2xx counts as delivered; otherwise the
delivery is retried with exponential backoff (10s, 20s, 40s … capped at 1h) until `max_attempts`, then marked `dead`.
Deliveries are persisted, so retries survive restarts; sent/dead rows are purged after 14 days.

### Messaging v2: templates, schedules, integrations, delivery log

| Route | Purpose |
|-------|---------|
| `GET/POST /admin/v1/templates`, `GET/PUT/DELETE /admin/v1/templates/{key}` | Template catalog: `format` (`plaintext` / `html` / `json`), `default_locale`, `translations` (`{ "en": { "title", "body" }, "de": … }`), `image_url`, `payload`. `GET` one includes `used_by` campaigns; a template in use cannot be deleted. |
| `POST /admin/v1/templates/{key}/render` `{ identity_id \| external_id, locale }` | Preview the rendered title/body for a player (locale: exact → language → default). |
| `POST /admin/v1/templates/{key}/test` `{ identity_id \| external_id, locale, push }` | Test send: renders and drops the message into that player's inbox (optionally pushes). |
| `GET/POST /admin/v1/integrations`, `GET/PUT/DELETE /admin/v1/integrations/{key}` | Delivery integrations: `kind` `webhook` (`url`, `secret`, `headers`), `onesignal` (`app_id`, `api_key`), `facebook` (`access_token`; player needs a `facebook_id` property), `fcm` / `apns` (server push config). Secrets are masked on read; sending the mask back keeps the stored value. |
| `POST /admin/v1/integrations/{key}/test` `{ external_id }` | Send a sample message through the integration; returns `{ status, detail }`. |
| `GET /admin/v1/messages/{key}/deliveries?status&channel&after&limit` | Per-channel delivery log (`sent` / `failed` / `skipped` with detail) plus `stats` per channel/status. |
| `DELETE /v1/inbox/{id}` | Remove a message from the player's inbox (unlike `DELETE /v1/messages/{id}`, which marks it consumed). |
| `DELETE /admin/v1/identities/{id}/inbox/{msg}` | Same, from the console. |

Campaign fields added: `template_key` (render from a template per player locale instead of inline title/body),
`channels` (`["inbox", "<integration key>", …]`; the inbox row is always the delivery record), and `schedule`:

```json
{ "kind": "once" }
{ "kind": "cron", "cron": "0 0 12 * * *", "window_secs": 86400 }
{ "kind": "live_event", "live_event": "weekend_rush", "anchor": "start", "offset_secs": 0, "window_secs": 86400 }
```

`cron` and `live_event` campaigns deliver once per occurrence (the inbox row carries `occurrence`), both on the
player's next inbox fetch and proactively by the housekeeping sweep every minute. Webhook deliveries are signed:
`X-Zanshin-Signature: sha256=HMAC_SHA256(secret, "{X-Zanshin-Timestamp}.{body}")`.

### Metrics catalog / Experiments v2

Metrics are named measurements over events: `kind` (`goal` | `monitor`), `event_name`, `aggregation`
(`count` | `unique` | `sum` | `avg` | `conversion`), optional `field` (metadata key for sum/avg).
Attach them to experiments via `experiment.metrics: ["key", …]`; results then report each metric per variant with lift vs control.

| Route | Purpose |
|-------|---------|
| `GET/POST /admin/v1/metrics`, `GET/PUT/DELETE /admin/v1/metrics/{key}` | Catalog CRUD (`GET` one includes `used_by` experiments). |
| `GET /admin/v1/experiments/{key}/results` | Now includes `metrics[]` (catalog metrics per variant + lift), `winner`, `interrupted_at`, `locked`, caps. |
| `POST /admin/v1/experiments/{key}/copy` `{ "key": "new", "as_phase": true }` | Duplicate as a draft (new salt, no assignments); `as_phase` records `phase_of` for sequences. |
| `POST /admin/v1/experiments/{key}/lock` / `unlock` | Locked experiments reject every change except status / lock. |
| `POST /admin/v1/experiments/{key}/interrupt` `{ "reason", "winner", "apply_to_flag" }` | Stop early, record winner; optionally write the winning value into the linked flag's default. |
| `GET /admin/v1/experiments/{key}/participants?variant&after&limit`, `…/participants.csv` | Who is in which variant, exposure state; CSV export. |
| `GET /admin/v1/experiments/{key}/series?metric&from&to` | Enrolment curve per variant per day; with `metric`, that metric's daily values per variant. |

Experiment fields added: `split_key` (`identity` or `property:<flat path>` — bucket by e.g. `custom.clan_id`),
`max_participants`, `enrol_until` (enrolment deadline; assigned players keep being served), `locked`, `metrics`,
`copied_from`, `phase_of`, `interrupted_at`, `interrupt_reason`, `winner`.

### Live events v3 / Experiments v3

| Route | Purpose |
|-------|---------|
| `variants[]` on a live event | `{ key, name, weight, payload?, flag_overrides }` — each participant gets one variant (deterministic by weight); `GET /v1/live-events` returns `variant` and the variant payload (falls back to the event payload); variant flag overrides apply on top of the event's. |
| `metrics[]` on a live event | Catalog metric keys evaluated over participants. Events sent while a player is in the event carry the tag `live:<key>` → variant (`default` without variants) in `experiments`, so every metric / filter that works per experiment variant works per live-event variant too. |
| `GET /admin/v1/live-events/{key}/metrics?from&to&metric` | Metric values per variant over the window (default: since the first occurrence) plus a daily `series` for `metric` (default: the first attached). |
| `GET /admin/v1/live-events/{key}/runs?limit` | Run report: past and current occurrences (newest first) with the attached metrics per run and per variant. |
| `GET /admin/v1/experiments/{key}/phases` | The experiment family (root + every experiment chained through `phase_of`), oldest first, with status / audiences / window / winner. |
| `GET /admin/v1/experiments/{key}/results.csv` | Results as CSV, one row per metric × variant (value, lift, credible interval, probability to beat control) plus assignment rows. |
| `srm` and `stats` on `GET /admin/v1/experiments/{key}/results` | `srm { chi2, p_value, suspicious }` compares assignment counts with the variant weights (chi-square; `suspicious` below p = 0.001). Conversion metrics carry `stats { rate, ci_low, ci_high, prob_beats_control, expected_uplift }` from Beta(1,1) posteriors (deterministic Monte Carlo); the control has no comparison fields. |

### Experiments v4: layers, holdouts, guardrails, bandits, sequential testing, CUPED

Experiment fields: `layer` (experiments sharing a layer are mutually exclusive — one bucket space split by
their `percentage`, which must sum to ≤ 100 across non-stopped experiments; 400 otherwise), `holdout` (key of a
holdout group whose players are never enrolled), `guardrails` (`[{ metric, max_drop (0..1), min_identities }]` — a
housekeeping job pauses the experiment with an `interrupt_reason` when a variant falls below
`control × (1 − max_drop)`; emits webhook `experiment.guardrail_tripped` and an `alert` notification), `bandit`
(`{ metric, min_weight, every_secs, min_assigned }` — Thompson sampling re-derives variant weights periodically;
`last_run_at` / `last_probabilities` are filled by the job; existing assignments stay sticky).

| Route | Purpose |
|-------|---------|
| `GET/POST /admin/v1/holdouts`, `GET/PUT/DELETE /admin/v1/holdouts/{key}` | Holdout groups: `percentage` (0..100), `salt`, `enabled`. Membership is a deterministic hash, so a player is held out consistently across every experiment referencing the group. Delete returns 409 while experiments reference it. |
| `GET /admin/v1/experiments/{key}/results?cuped=true&pre_days=14` | Adds `cuped`: per count / sum / avg metric, θ, variance reduction and raw vs adjusted means per variant using each player's pre-experiment value of the metric as covariate. Conversion metrics also carry `sequential` (mSPRT always-valid p-value, `significant`, `lift`) next to the Bayesian `stats`. |

### Core events / Analytics v4

- Core catalog: `GET /admin/v1/taxonomy/events?core=true|false&synthetic=true|false&discovered=true&q=`.
  Core definitions are immutable in shape — `PUT` with a different `value_type` or a
  `metadata_schema` returns `400`; `description`, `tags`, `exclude_from_activity` can change.
- Currency rates (`analytics:*`): `GET /admin/v1/currency-rates?since=YYYY-MM-DD` → `{items,
  currencies, remote_configured}`; `PUT /admin/v1/currency-rates {day?, rates: {EUR: 1.08}}`
  (USD per unit); `POST /admin/v1/currency-rates/refresh` pulls the configured feed
  (`taxonomy.currency_rates_url`, USD base, `{"date", "rates": {code: units per USD}}`);
  `GET /admin/v1/currency-rates/convert?amount&currency&day` → `amount_usd` (null if unknown).
  At ingest an event with `metadata.amount` + `metadata.currency` gets `metadata.amount_usd`
  using the latest rate on or before its day.
- Computed core properties: `purchases`, `spend_usd`, `last_purchase_at`, `ad_impressions`,
  `ads_completed`, `ad_revenue_usd`, `currency.<code>.granted` / `.spent`.
- Test flag: `metadata.test = true` tags the event `$test`; it is ignored by computed properties
  and hidden from all reports unless the filter has `include_test: true`.
- Funnels v2: `POST /admin/v1/analytics/funnel` accepts `count` (`unique` — once per player, deepest
  chain; `total` — every entry) and `cohort` (`static` — entered inside the range; `shifting` —
  entered up to one window earlier but reached a step inside the range).
- Retention by operation: pass `experiment=live:<key>` (and optional `variant`) or
  `experiment=<key>` to `GET /admin/v1/analytics/retention`.
- Lifetime engagement: `GET /admin/v1/analytics/lifetime?from&to` → `{players, averages:
  {sessions, play_time_sec, spend_usd, purchases, events, lifetime_days, payers_pct, arppu_usd},
  buckets: {sessions, play_time, lifetime_days, spend_usd}}` for players created in the range.
- Explore overlay: `POST /admin/v1/analytics/explore {events: [...], ...}` (≤ 8) → `{series:
  [{event, result}]}` with the same range / granularity / filter for every event.

### Flags v2 / Live events v2

Override chain for a flag value: **identity override → experiment variant → live-event flag override →
rules in order (audience-scoped variants, rollout) → default**. `GET /admin/v1/flags/{key}/explain/{identity_id}`
returns every layer with what matched and the final result (`reason` may now be `live_event`).

Live event fields: `join_mode` (`auto` | `explicit`), `sticky`, `participation_secs`, `flag_overrides`
(`{ "flag_key": value }` applied to participants while the event is active for them).

| Route | Purpose |
|-------|---------|
| `POST /v1/live-events/{key}/join`, `POST /v1/live-events/{key}/leave` | Explicit participation (also allowed for `auto` events to start a participation timer). Join requires an open window and matching targeting; emits `$live_event_join`. |
| `GET /v1/live-events` | Per-player view now includes `join_required`, `joined_at`, `participation_ends_at`. Sticky participants keep the event until their timer ends even outside a window / after targeting changes. |
| `GET /admin/v1/live-events/{key}/participants?after&limit` | Who joined and when (`left_at` when they left). |
| `GET /admin/v1/live-events/{key}/timeline?from&to` | Occurrences in range, joins per day, participant totals. |
| `GET` / `PUT` / `DELETE /admin/v1/identities/{identity_id}/qa-time` `{ "at": "2026-12-24T18:00:00Z" }` | QA time travel: pin the evaluation clock for one identity (7 days) so future live-event windows and schedules can be tested; affects live events and the flag explain view. |

### Audiences v2

Audience fields beyond the rule: `kind` (`dynamic` | `managed`), `recompute` (`live` | `scheduled`),
`schedule_cron` (6-field cron, UTC), `source`, `copied_from`, `last_computed_at`, `member_count`.
Create / update accept `expression` instead of `condition` (see *Expression language*). Membership is
resolved per audience as **exclude list → include list → stored membership (managed / scheduled) → live rule**.

| Route | Purpose |
|-------|---------|
| `DELETE /admin/v1/audiences/{key}/members/all` | Permanently deletes every identity in the audience (GDPR delete per member, events included). Only with `admin.audience_delete_all_enabled: true` on a non-production server and the `identities:delete` permission; otherwise 403. |
| `GET /admin/v1/audiences/{key}/members?after&limit` | Current members with `joined_at` and `source` (`rule` / `include` / `managed`). Live audiences mirror rule results here as identities are evaluated. |
| `POST` / `DELETE /admin/v1/audiences/{key}/members` `{ "external_ids": [...] }` | Managed membership (admin). Same on the server API: `POST` / `DELETE /server/v1/audiences/{key}/members` (server key, e.g. from Nakama). Unknown ids are returned in `missing`. |
| `GET` / `PUT` / `DELETE /admin/v1/audiences/{key}/lists` | Include / exclude entries `{ "external_id", "mode": "include"|"exclude", "until"?, "reason"? }`; exclusions with `until` expire automatically. |
| `POST /admin/v1/audiences/{key}/recompute` | Re-evaluate the rule for every identity and store the result; scheduled audiences also run on `schedule_cron`. |
| `GET /admin/v1/audiences/{key}/usage` | Flags, experiments, live events and messages referencing the audience. |
| `POST /admin/v1/audiences/{key}/copy` `{ "key", "name"? }` | Duplicate rule / sampling / labels as a disabled audience (`copied_from`). |
| `GET /admin/v1/audiences/{key}/expression`, `POST /admin/v1/audiences/expression` | Convert between the expression language and the JSON condition. |

Default audiences: `all_players`, `new_players` (first seen ≤ 7 d), `payers`, `inactive_7d`, `ios`, `android`.

#### Expression language

```text
custom.level >= 10 and (default.country in ["DE", "FR"] or computed.session_count > 3)
  and not custom.beta == true and computed.last_seen_at within_last 7d
```

Precedence `not` > `and` > `or`, parentheses group. Operators: `== != < <= > >= in not_in contains
not_contains starts_with ends_with matches exists not_exists semver_* within_last older_than`; durations
`30s 15m 2h 7d`. The new rule operators `within_last` / `older_than` (value in seconds) work on any
timestamp property and are available everywhere conditions are.

### Taxonomy

| Route | Purpose |
|-------|---------|
| `GET/POST /admin/v1/labels`, `PUT/DELETE /admin/v1/labels/{id}`, `GET /admin/v1/labels/{id}/usage` | Labels registry. Renaming / deleting a label rewrites the `tags` of every flag, audience, experiment, live event, message and event definition. All list endpoints accept `?tags=a,b` (every tag must match). |
| `GET/POST /admin/v1/taxonomy/events`, `GET/PUT/DELETE /admin/v1/taxonomy/events/{name}` | Event definitions: `value_type` (`any` / `number` / `string` / `object` / `revenue` — requires `metadata.amount` + `metadata.currency`), optional `metadata_schema` (JSON Schema), `exclude_from_activity`, `tags`. Core events (`sessionStart`, `purchaseCompleted`, `levelCompleted`, …) are seeded and cannot be deleted. |
| `GET/PUT /admin/v1/taxonomy/settings` | `mode`: `off` (no checks) · `warn` (store, report) · `strict` (reject with 400 / drop in batches); `auto_discover` registers unknown event names as `discovered` definitions. |
| `GET/DELETE /admin/v1/taxonomy/rejected?limit=` | Last N (default 200) invalid events with identity, payload and reason, and whether they were dropped. |
| `GET /admin/v1/taxonomy/properties`, `PUT/DELETE /admin/v1/taxonomy/properties/{key}` | Observed property catalog per namespace plus configuration; `read_only: true` makes a custom key writable only through the server API. |
| `GET/POST /admin/v1/schemas`, `GET/PUT/DELETE /admin/v1/schemas/{id}`, `POST /admin/v1/schemas/validate` | JSON Schemas for structured values; `validate` takes `{ "schema", "samples": [...] }` and returns per-sample errors. |
| `POST /admin/v1/schemas/presets` | Creates the built-in validators that are missing — `ANY`, `Number`, `String`, `Object`, `Event-Revenue-Metadata` (`amount` + ISO-4217 `currency`, optional `test`). Idempotent. |
| `GET /admin/v1/identities/{id}/sessions`, `GET /admin/v1/identities/{id}/merges` | Play-session and merge history. |

Configuration: `taxonomy.mode`, `taxonomy.auto_discover`, `taxonomy.geoip_db`, `taxonomy.session_inactivity`,
`taxonomy.rejected_keep` (env `ZANSHIN__TAXONOMY__*`).

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
| `GET /admin/v1/system` | Version, build info, store health, inventory counts, limits and `console.audience_delete_all_enabled`. |
| `GET /admin/v1/system/config` | Effective configuration (defaults → `zanshin.yml` → `ZANSHIN__*` environment) with secrets masked (`***`, user-info stripped from URLs); Settings → Configuration, exportable as JSON. |
| `GET/PUT /admin/v1/console/settings` | Console-wide display preferences shared by all users: `timezone` (IANA name, empty = browser) and `default_range_days`. |

gRPC: `grpc.health.v1.Health/Check`, `grpc.reflection.v1.ServerReflection`.

## Data export and management

### Export sinks

Ship raw events to external storage. Each sink keeps a `(received_at, id)` cursor and a run log; the housekeeping loop runs due sinks every minute, and a full batch immediately schedules the next one.

| Method | Path | Notes |
|---|---|---|
| GET/POST | `/admin/v1/export-sinks` | `kind`: `s3` (`bucket`, `region`, `access_key`, `secret_key`, optional `endpoint` for S3-compatible stores, `prefix`), `http` (`url`, optional `secret` → `X-Zanshin-Signature`, `headers`), `bigquery` (`project`, `dataset`, `table`, `service_account`). `format`: `jsonl` \| `csv`, `gzip`, `events` filter (empty = all), `interval_secs`, `batch_size`. Secrets are masked (`***`) on read and kept when the mask is sent back. |
| GET/PUT/DELETE | `/admin/v1/export-sinks/{key}` | |
| POST | `/admin/v1/export-sinks/{key}/test` | Delivers one synthetic `$export_test` event. |
| POST | `/admin/v1/export-sinks/{key}/run` | Runs one batch now and returns the run. |
| GET | `/admin/v1/export-sinks/{key}/runs?limit` | Run log (`ok` / `empty` / `failed`, rows, bytes, object). |
| POST | `/admin/v1/export-sinks/{key}/reset-cursor` | `{ "to": timestamp \| null }` — re-export from a point in time (or from the beginning). |

### Data lakes v2

- `format: parquet` (snappy, `gzip` ignored) for S3-backed sinks (`s3`, `snowflake`, `redshift`);
  `config.partition = "event"` writes one object per event name:
  `<prefix>/event=<name>/YYYY/MM/DD/<stamp>-<id>.<ext>`. Rejected events (`$event_error`) carry a
  top-level `_error` column with the validation reason in every format.
- `kind: snowflake` — files are staged in the sink's bucket, then `COPY INTO
  <database>.<schema>.<table> FROM @<stage> FILES = (…)` runs through the SQL API
  (`https://<host>/api/v2/statements`) with key-pair JWT auth. Config: S3 keys + `account`,
  `host`, `user`, `private_key` (PEM), `public_key_fp` (`SHA256:…` from `DESCRIBE USER`),
  `database`, `schema`, `table`, `stage` (external stage pointing at bucket + prefix), optional
  `warehouse`, `role`.
- `kind: redshift` — files are staged in S3, then `COPY <table> FROM 's3://…' IAM_ROLE '…'` runs
  through the Redshift Data API (SigV4 with the sink's AWS keys). Config: S3 keys + `database`,
  `table`, `iam_role`, exactly one of `cluster_id` / `workgroup`, optional `db_user`, `secret_arn`.

### Notification channels (`integrations:*`)

`GET/POST /admin/v1/notification-channels`, `GET/PUT/DELETE /admin/v1/notification-channels/{key}`,
`POST …/{key}/test`, `GET /admin/v1/notification-channels/topics`. Body: `kind` (`slack` incoming
webhook, `discord` webhook, `webhook` any HTTPS endpoint receiving `{topic, title, text, kind,
fields, at}`), `url`, `topics` (`*`, `config.changed`, `config.deleted`, `export.failed`,
`integration.unhealthy`, `alert`, `test`), `kinds` (limit config topics to entity kinds such as
`flags`, `experiments`), `enabled`, `headers` (webhook only). Slack / Discord URLs are masked when
read back; `last_status` / `last_error` / `last_sent_at` show the latest delivery. Config
changes are derived from successful mutating admin calls (actor, method, path).

### Configuration bundles

| Method | Path | Notes |
|---|---|---|
| GET | `/admin/v1/config/kinds` | Entity kinds and bundle format. |
| GET | `/admin/v1/config/export?kinds=flags,audiences` | Bundle `{ format: "zanshin-config/1", exported_at, entities: { kind: [...] } }` with volatile fields (ids, versions, timestamps) stripped. |
| POST | `/admin/v1/config/diff` | `{ bundle, mode: merge \| replace, kinds? }` → per kind `added` / `changed` (with field names) / `removed` (replace only) / `unchanged`. |
| POST | `/admin/v1/config/import` | `{ bundle, mode, kinds?, dry_run }` → the plan plus `applied` and per-entity `errors`. |
| POST | `/admin/v1/config/import-from` | `{ url, admin_key, mode, kinds?, dry_run }` — pulls `/admin/v1/config/export` from another Zanshin server and imports it. |
| GET | `/admin/v1/config/history?kind&key&limit` | Snapshots of every configuration change (bus-driven for flags/audiences/experiments/live events/messages, plus everything touched by imports and reverts). |
| POST | `/admin/v1/config/history/{id}/revert` | Restore the entity to that snapshot (or delete it again). |
| POST | `/admin/v1/audit/{id}/undo` | Undo the change recorded by an audit entry: restores the last snapshot before it, or deletes an entity the entry created. |

### Data tools

| Method | Path | Notes |
|---|---|---|
| GET | `/admin/v1/data/identities/{id}/takeout` | Identity, properties, recent events and play sessions as one JSON document. |
| DELETE | `/admin/v1/data/identities/{id}` | GDPR deletion: identity, sessions, inbox, assignments and all events (ClickHouse mutation). |
| POST | `/admin/v1/data/events/purge` | `{ before, names? }` — delete events older than a timestamp (at least one hour in the past). |
| POST | `/admin/v1/data/sample` | `{ players, days, seed }` — deterministic synthetic players with sessions, level-ups and purchases for trying analytics on an empty server. |
| POST | `/admin/v1/data/defaults` | Re-creates the default audiences (`tier_1_countries`, `spenders`, `whales`, `non_spenders`, `new_players`, `lapsed_players`, `ios`, `android`) and metrics (`active_users`, `sessions`, `revenue`, `purchases`, `paying_users`, `ad_impressions`) that are missing; existing keys are untouched. |

## Console users and permissions

Admin access is authorised by **permissions**: a map `resource -> [read, write, delete]` over
`flags, audiences, experiments, metrics, live_events, messages, templates, integrations, webhooks,
journeys, identities, taxonomy, analytics, data, audit, api_keys, users, settings`. The key `*`
applies to every resource that has no explicit entry (so `{"*": ["read"], "flags": ["read","write"]}`
is "read everything, edit flags"). `write` implies `read`. Every `/admin/v1` route maps to one
resource by its first path segment and to an action by method (`GET` → read, `DELETE` → delete,
otherwise write); a refused call answers `403` with `message: "permission required: <resource>:<action>"`.
Legacy roles still work: `admin` = everything, `editor` = everything except writing `api_keys` /
`users`, `viewer` = read-only.

`GET /admin/v1/permissions` — resources, actions and the archetype presets with their grants:
`admin`, `developer`, `game_designer`, `debugger`, `support`, `analyst`, `viewer`.

### Users (`users:*`)

| Method | Path | Notes |
| --- | --- | --- |
| `GET` | `/admin/v1/users` | list (never includes hashes or secrets) |
| `POST` | `/admin/v1/users` | `{email, name, password?, archetype?, permissions?}` — without `password` the response carries a one-time `reset_token` |
| `GET` / `PATCH` / `DELETE` | `/admin/v1/users/{id}` | `PATCH {name?, archetype?|null, permissions?, disabled?}`; permission changes revoke the user's sessions |
| `POST` | `/admin/v1/users/{id}/reset-link` | `{reset_token, path, expires_in_secs}` (24 h, single use) |
| `DELETE` | `/admin/v1/users/{id}/mfa` | remove the user's authenticator (lost device) |
| `DELETE` | `/admin/v1/users/{id}/sessions` | sign the user out everywhere |

### Login (no API key)

* `POST /console/auth/password` `{email, password, code?}` → sets the `zanshin_console_user`
  cookie (HttpOnly, SameSite=Lax, Secure behind TLS). When the user has MFA and no `code` is given
  the answer is `401 {"code": "mfa_required"}`; `code` accepts a TOTP or a recovery code.
* `POST /console/auth/reset` `{token, password}` — completes a reset link.
* `POST /console/auth/logout` — revokes the session and clears the cookie.
* `GET /console/auth/config` → `{oidc, password, needs_bootstrap}`; with `needs_bootstrap` the
  console offers to create the first admin using the bootstrap key.

Passwords: Argon2id, at least 12 characters, common words rejected; an initial password set by an
admin must be changed on first login (`must_change_password` in `GET /admin/v1/me`).

### Own account (`/admin/v1/me/*`, console users only)

| Method | Path | Notes |
| --- | --- | --- |
| `GET` | `/me` | now also `permissions`, `user_id`, `mfa_enabled`, `must_change_password`, `auth: "user"` |
| `POST` | `/me/password` | `{current_password, new_password}`; revokes other sessions |
| `POST` | `/me/mfa/setup` | `{secret, otpauth_url}` (RFC 6238, SHA-1, 30 s, 6 digits) |
| `POST` | `/me/mfa/enable` | `{code}` → `{recovery_codes: [10 single-use codes]}` |
| `POST` | `/me/mfa/disable` | `{code}` |
| `GET` | `/me/sessions` · `DELETE /me/sessions/{id}` | active sessions with IP / user agent |

### API keys

`POST /admin/v1/api-keys` accepts `permissions` or `archetype` for admin keys (they override
`role`). `POST /admin/v1/api-keys/{id}/rotate` `{grace_hours?: 24}` mints a successor with the same
name, kind, limits and grants and gives the old key `expires_at = now + grace`; the rows link
through `rotated_from` / `rotated_to`, and a key rotates at most once.
||||||| constructed merge base


## Analytics v3

Every event-based report under `/admin/v1/analytics` accepts a **dimension filter** as query
parameters (or a `filter` object in POST bodies): `country`, `platform`, `app_version`,
`experiment` + `variant`. Dimensions are copied onto each event at ingest from the identity's
default properties (`country`, `platform`, `app_version`), so the filter is exact for events
recorded after this release and matches nothing older. `variant` requires `experiment`.

| Method | Path | Notes |
| --- | --- | --- |
| `GET` | `/analytics/status` | online now (open play sessions, 5 min), DAU today, events last hour, running experiments / live events / campaigns / journeys, live events in the next 7 days, 24 h errors |
| `GET` | `/analytics/calendar?from&to` | items `{kind: live_event\|experiment\|campaign\|annotation, key, name, start, end, open_ended}` + daily `metrics` (DAU / revenue) for the range; default −7 … +14 days |
| `POST` | `/analytics/explore` | `{event, from?, to?, granularity: day\|hour, group_by?: country\|platform\|app_version\|experiment:<key>\|meta:<field>, sum_field?, filter?}` → `total`, `groups`, `series` |
| `GET` | `/analytics/map` | players / events / share per country |
| `GET` | `/analytics/playtime` | per-day sessions, players, total / avg / median seconds; totals; session-length histogram |
| `GET` | `/analytics/retention?cumulative=true` | `cumulative` counts players who returned on **or before** day N; response adds `installs` |
| `POST` | `/analytics/funnel` | `model`: `window` (default; all steps within `window_secs` of the first), `step` (each within `window_secs` of the previous), `session` (same play session), `unlimited` |
| `GET` | `/analytics/ltv?days=1,7,30` | install cohorts × `{revenue, ltv, roas}` at day offsets, `installs`, `spend`, `cpi` (spend from ad-spend rows of the same day, narrowed by platform / country when filtered) |
| `GET` / `POST` / `DELETE` | `/analytics/ad-spend` | spend rows keyed by `(day, platform, country, campaign)`; `POST {rows: [...]}` and/or `{csv: "day,platform,country,campaign,spend,installs[,currency]"}` upserts |
| `GET` | `/analytics/errors?from` | rejected events (count, by reason, recent) and failed `message_deliveries` / `webhook_deliveries` / `export_runs` since `from` (default 24 h) |

`GET /analytics/overview` and the dashboard `number` / `timeseries` / `funnel` / `retention`
widgets apply the same filter; a dashboard's `filters` object is used as its event filter.
