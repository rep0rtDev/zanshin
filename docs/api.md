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
| `WS` | `/stream` (`x-api-key` header or `?api_key=`) | change feed: `{"type":"change","kind":"flag\|audience\|experiment\|live_event\|message","key","version","deleted","identity_id"}`; `{"type":"resync"}` after lag; `ping`/`pong` |

gRPC: `ServerGetFlags`, `ServerGetAudiences`, `ServerGetExperiments`, `ServerGetLiveEvents`, `ServerSendMessage`, `ServerSendEvents`.

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

### Journeys

Per-player automations under `/admin/v1/journeys` and the **Journeys** console page.

| Route | Purpose |
|-------|---------|
| `GET/POST /admin/v1/journeys`, `GET/PUT/DELETE /admin/v1/journeys/{key}` | Journey definition: `trigger`, entry `condition` / `audiences`, `steps`, `cooldown_secs`, `max_entries`, `starts_at` / `ends_at`. |
| `POST /admin/v1/journeys/{key}/enter` `{ identity_id \| external_id }` | Enter a player now (honours targeting, cooldown and caps; ignores the live window). |
| `GET /admin/v1/journeys/{key}/runs?status&after&limit` | Runs with per-status stats and a per-step count of players currently waiting. |
| `GET /admin/v1/journeys/{key}/runs/{id}` | One run with its step log. |
| `POST /admin/v1/journeys/{key}/runs/{id}/exit` | Stop an active run. |
| `GET /admin/v1/identities/{id}/journeys` | A player's runs across journeys. |

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
analytics event, and `<flag|audience|experiment|live_event|message>.<updated|deleted>` for configuration changes.

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

gRPC: `grpc.health.v1.Health/Check`, `grpc.reflection.v1.ServerReflection`.
