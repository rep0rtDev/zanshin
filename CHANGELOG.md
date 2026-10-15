# Changelog

All notable changes to Zanshin are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [SemVer](https://semver.org).
Release automation reads the section whose heading contains the tagged version.

## [Unreleased]

## [0.1.0] — 2026-10-15

First release: the complete LiveOps platform — flags, audiences, experiments, live events,
journeys, messaging, analytics, data lakes, multi-project tenancy, SSO (OIDC / SAML / SCIM),
config as code, SDKs for 12 runtimes and the Nakama module. Highlights of the final round:

### Added
- Ops: `restore-drill.sh` in the backup kit restores a dump into a scratch database and verifies
  schema version + row counts; CI runs the full backup → drill → `zanshin --doctor` loop on every
  push. Backup offsite mirror now uses `aws s3 sync` (any S3-compatible store, e.g. RustFS).
  `docs/production.md` gained the step-by-step upgrade procedure, secret rotation table, degraded
  mode matrix and an incident checklist.
- Hardening: `zanshin --doctor [--json]` checks a deployment end to end (config sanity, PostgreSQL +
  pending migrations, Redis, ClickHouse, clock skew, integrations, console users, SSO) and exits 1
  on failures; CI job `live` runs the suite against real services — Keycloak (OIDC **and** SAML
  logins with group → role mapping), ClickHouse (SQL explorer, paths, alerts), Mailpit (SMTP),
  RustFS (S3 jsonl + Parquet export) and Nakama with the Zanshin module (RPCs, identity sync);
  SCIM request shapes of Okta and Microsoft Entra ID are replayed in e2e.
- Platform — SDKs: **local evaluation** of flags in the Go and TypeScript server SDKs from a new
  rules bundle (`GET /server/v1/rules`: flag definitions + dynamic audiences; same xxh3 bucketing,
  operators and reasons as the server; refreshed on TTL and change-stream events — zero-latency
  reads; overrides / experiments / live events stay server-side), **OpenFeature providers** for
  Go (`sdk/go/openfeature`, `open-feature/go-sdk`) and TypeScript (`ZanshinProvider`, structural
  types, remote or local mode), and **Nakama zero-code core events**: the Nakama module mirrors
  validated purchases (Apple / Google / Huawei / Facebook Instant → `purchaseCompleted`),
  leaderboard and tournament record writes (`eventLeaderboardScore`) and friend invites
  (`inviteSent`) into Zanshin without client changes (`ZANSHIN_CORE_EVENTS=false` to disable).
- Platform — identity & config: **SAML 2.0** console login (SP-initiated, HTTP-POST binding,
  XML-DSig verification with exclusive C14N and RSA-SHA256 — no libxml; `admin.saml`, routes
  `/console/auth/saml/{login,acs,metadata}`), **SCIM 2.0** provisioning of console users
  (`/scim/v2` Users + Groups → archetypes, bearer token `admin.scim.token`), **config as code** in
  `zanshinctl` (`config pull <dir>` / `config apply <dir> [--prune] [--dry-run]` /
  `config drift <dir>` exits 2 on drift; `<kind>/<key>.yaml`) with the composite GitHub Action
  `.github/actions/zanshin-apply`, and **environment promotion** from the console / API
  (`POST /admin/v1/config/promote` pushes this project's configuration into another server with a
  dry-run plan).
||||||| 8fbff05
- Platform — identity & config: **SAML 2.0** console login (SP-initiated, HTTP-POST binding,
  XML-DSig verification with exclusive C14N and RSA-SHA256 — no libxml; `admin.saml`, routes
  `/console/auth/saml/{login,acs,metadata}`), **SCIM 2.0** provisioning of console users
  (`/scim/v2` Users + Groups → archetypes, bearer token `admin.scim.token`), **config as code** in
  `zanshinctl` (`config pull <dir>` / `config apply <dir> [--prune] [--dry-run]` /
  `config drift <dir>` exits 2 on drift; `<kind>/<key>.yaml`) with the composite GitHub Action
  `.github/actions/zanshin-apply`, and **environment promotion** from the console / API
  (`POST /admin/v1/config/promote` pushes this project's configuration into another server with a
  dry-run plan).
- Analytics v5: read-only **SQL explorer** over ClickHouse (`POST /admin/v1/sql`, permission `sql`,
  single-statement guard, `readonly=2`, row / time caps), **saved queries**, **scheduled reports**
  (cron + timezone, saved query → Slack / Discord / webhook channels and/or email through an email
  integration), **metric alerts** (count / unique / sum of an event over a window; above / below
  threshold or z-score anomaly vs previous windows; cooldown, fire history, `alert` notifications),
  **path analysis** (`POST /admin/v1/analytics/paths`: what players do after / before an event),
  **cohort → audience** from a funnel step (`POST /admin/v1/analytics/funnel/cohort`) or a SQL
  result (`POST /admin/v1/sql/cohort`) as managed audiences, and the unified **player timeline**
  (`GET /admin/v1/identities/{id}/timeline`: events, sessions, message deliveries, journeys).
  Console: Analytics → Explorer (SQL / Saved queries / Paths / Alerts / Reports), "Save as
  audience" on funnels, Timeline tab on the player page.
- Messaging v3: native **email** integrations (`smtp` with STARTTLS / implicit TLS / AUTH LOGIN,
  `ses` via SESv2 + SigV4, `sendgrid`, `resend`; recipient from `to_prop`, default
  `default.email`), per-campaign **frequency caps**, **quiet hours** in the player's timezone
  (deferred deliveries retried by housekeeping, `GET /messages/{key}/deferrals`), sticky weighted
  **copy variants** (A/B titles / bodies / templates, `GET /messages/{key}/variants` with
  sent / read per variant) and **rich push** options (sound, badge, channel, collapse key, deep
  link, action buttons) on campaigns and direct messages, mapped to FCM / APNs payloads.
- Journeys v3: **Split** step (weighted random branching, sticky per run, branch recorded on the
  run), **goal** event with optional deadline (runs marked converted; `conversions` in the runs
  report, per branch), **exit criteria** (`exit_when.events`, `exit_when.on_goal` end active runs).
- Flags v3: **prerequisites** between flags (dependency must be enabled / at a value, cycles
  rejected, `reason: prerequisite`), one-click **kill switch** (`/kill`, `/restore`), **scheduled
  changes** applied by housekeeping, **evaluation counters** per flag / variant with a
  **stale-flag** report, and **change requests with approvals** (`/admin/v1/change-requests`,
  permission `approvals`, four-eyes rule) for every config kind; console: prerequisites editor,
  Schedule and Stats tabs, kill switch, stale badges, Change requests page.
- Experiments v4: mutually exclusive **layers**, **holdout groups** (`/admin/v1/holdouts`) that keep a
  deterministic slice of players out of experiments, **guardrail metrics** that pause an experiment
  automatically when a variant regresses control beyond a tolerance (webhook
  `experiment.guardrail_tripped`, `alert` notification), **bandit allocation** (Thompson sampling
  re-weights variants towards the likely winner, sticky assignments untouched), **sequential testing**
  (always-valid mSPRT p-values on conversion metrics, safe to peek at any time) and **CUPED**
  variance reduction (`?cuped=true`) on count / sum / avg metrics; console form, results panel and
  holdouts card.
- Satori parity leftovers: **Flags** tab on an identity (`GET /admin/v1/identities/{id}/flags`) showing every
  resolved flag with its value source (default / rule / variant / override / experiment / live event); the
  expression ("query view") editor is now available on every condition — flag rules, live events, journeys,
  messages and experiments, not only audiences; built-in validators (`POST /admin/v1/schemas/presets`:
  `ANY`, `Number`, `String`, `Object`, `Event-Revenue-Metadata`); Settings → General console preferences
  (`GET/PUT /admin/v1/console/settings`: display timezone for all users, default analytics range) and a
  Settings → Configuration tab with the effective, secret-masked server configuration and JSON export
  (`GET /admin/v1/system/config`); default data restore (`POST /admin/v1/data/defaults`: Tier-1-Countries,
  Spenders, Whales, New-Players, Lapsed-Players, iOS / Android audiences and Active-Users, Sessions, Revenue,
  Purchases, Paying-Users, Ad-Impressions metrics); **Delete all identities** on an audience, gated by
  `admin.audience_delete_all_enabled` and refused in production; webhook event types `flag.rule.created` /
  `flag.rule.updated` / `flag.rule.deleted`, `journey.step_reached` and `message.direct_sent`.

### Changed
- Console cleanup (same design, tidier): denser spacing for cards, tables, forms and stats; detail
  pages are roughly a third of their former height; audience and metric chip walls replaced by a
  searchable picker (`KeyPicker`) with removable chips; the Status board caps each list at 8 rows
  with a "+N more" link and keeps dates on one line; breadcrumbs no longer wrap and the project
  switcher is sized to its content; JSON editors grow with their content instead of a fixed height;
  range/checkbox accents follow the theme; removed a stray merge marker and a duplicate `.tabs` rule.

### Added
- gRPC follows project routing: the `x-zanshin-project` metadata key, else the `prj` claim of the
  session token, selects the project (default otherwise); unknown projects answer `NOT_FOUND`.
- Swagger UI on `/docs` is vendored and served from the binary (no unpkg CDN); the page works
  offline and behind a strict CSP.
- Multi-project: several isolated games / environments in one deployment. Every non-default project
  gets its own PostgreSQL schema (`zanshin_<slug>`, migrated on creation), Redis namespace and
  ClickHouse database, with its own configuration, players, events, API keys and housekeeping;
  console users and SSO are shared. Requests are routed by the `x-zanshin-project` header,
  `?project=`, or the `prj` claim stamped into session tokens (bare tokens keep working), default
  otherwise. `GET/POST /admin/v1/projects`, `GET/PUT /admin/v1/projects/{slug}` (archive / resume),
  project switcher in the console top bar and Settings → Projects, `zanshinctl --project` /
  `config promote --to-project`. Existing data stays in the `default` project (public schema).
- SDK wave 3 + tooling: Godot 3.5+ addon (`sdk/godot3`), Defold Lua SDK (`sdk/defold`), header-only
  C++17 SDK with libcurl adapter (`sdk/cpp`), OpenAPI 3 document (`docs/openapi.yaml`, served at
  `/openapi.yaml` / `/openapi.json`) with Swagger UI on `/docs`, and the `zanshinctl` CLI
  (`crates/zanshin-cli`: config export / diff / import / promote between servers, history / revert,
  API keys, player lookup / events / flags / GDPR delete, flags, export sinks, raw API calls) shipped
  alongside the server binaries.
- Data lakes v2: Parquet export format (snappy; Databricks / Athena / Spark), `partition: event`
  (one object per event name under `event=<name>/`), Snowflake sink (S3 stage + `COPY INTO`
  through the SQL API with key-pair JWT), Redshift sink (S3 + `COPY` through the Data API,
  provisioned or serverless), `_error` column on exported `$event_error` rows (JSONL / CSV /
  Parquet / BigQuery). Notification channels (`/admin/v1/notification-channels`): Slack, Discord
  or generic webhook targets subscribed to `config.changed` / `config.deleted` (optionally only
  some entity kinds), `export.failed`, `integration.unhealthy`, `alert`; test send, delivery
  status, console page Platform → Notifications.
- Core events + analytics v4: the rest of the standard core catalog (`adImpression`,
  `currencyGranted/Spent`, `itemGranted/Spent/Consumed`, `eventLeaderboardScore/Rank`,
  `achievementUnlocked`, `questStarted/Completed`, `inviteSent/Accepted`, `socialShare`,
  `appUpdated`) with immutable schemas (value type and metadata schema of core events can't be
  changed; description/tags can), daily currency → USD rates (`/admin/v1/currency-rates` manual
  table, optional feed `taxonomy.currency_rates_url` polled by housekeeping, `metadata.amount_usd`
  filled at ingest), computed core properties per player (`purchases`, `spend_usd`,
  `last_purchase_at`, `ad_impressions`, `ads_completed`, `ad_revenue_usd`,
  `currency.<code>.granted/spent`), `metadata.test = true` marks QA events (excluded from computed
  properties and from every report unless the filter sets `include_test`), taxonomy list filters
  `core` / `synthetic` / `discovered` / `q` with Show core / Show synthetic toggles in the console,
  funnels v2 (`count: unique|total`, `cohort: static|shifting`), retention by operation (live event
  / experiment participants) in the console, `GET /admin/v1/analytics/lifetime` (lifetime
  engagement: sessions / play time / lifetime / spend distributions, payer share, ARPPU) and
  multi-event Explore overlay (`events: [...]` → one series per event).
- Live events v3: variants inside a live event (deterministic per-player assignment by weight,
  per-variant payload and flag overrides on top of the event's), monitor metrics attached to an
  event (`metrics`), analytics events tagged `live:<key>` → variant while the player is in the
  event, `GET /admin/v1/live-events/{key}/metrics` (per-variant values + daily series) and
  `GET /admin/v1/live-events/{key}/runs` (run report: every occurrence with its metrics, side by
  side), Monitor / Run report panels and a variants editor in the console.
- Experiments v3: phase families (`GET /admin/v1/experiments/{key}/phases`, Phases panel), results
  as CSV (`GET /admin/v1/experiments/{key}/results.csv`), Bayesian conversion statistics per
  variant (posterior rate, 95% credible interval, probability to beat control, expected uplift)
  and a sample-ratio-mismatch check (`srm`: chi-square p-value, flagged below 0.001) on
  `GET /admin/v1/experiments/{key}/results`, both shown in the console.
- Journeys v2: derived lifecycle `status` (disabled / scheduled / running / terminated), `time_window`
  steps (a date, or the next slot of a cron schedule resolved in UTC, a fixed zone or the player's own
  timezone), `live_event` steps that start a live event on the player's own timeline (personal
  window, independent of the event schedule), `revert` steps that undo an earlier flag override /
  experiment assignment / live event, `enter_journey` steps with cycle protection, per-step
  "passed" counts and fastest / average / slowest time-to-finish on the runs endpoint, a player's
  journey history with step logs (`GET /admin/v1/identities/{id}/journeys`, Journeys tab on the
  player page) and a flow canvas in the console (auto-laid-out graph with branch / goto edges and
  live player counts per step).
- SDK wave 2: gRPC parity with REST (`Identify`, `GetFlagOverrides`, `JoinLiveEvent` / `LeaveLiveEvent`,
  `RemoveMessage`, push-token RPCs, and server-key RPCs for single flag / explain / identity upsert /
  identify / properties / live-event join-leave / inbox / managed audience members; gRPC `Authenticate`
  now opens a play session like HTTP), Nakama runtime modules for the JavaScript (`nakama/ts`) and Lua
  (`nakama/lua`) runtimes with the same RPCs as the Go plugin, new client SDKs for Swift (`sdk/swift`),
  Kotlin / JVM (`sdk/kotlin`) and Dart / Flutter (`sdk/dart`) with the full client surface and the
  live stream, and a LiveOps recipes library in the console (timed offer, dynamic pricing, offer wall,
  return incentive, soft launch, holiday event) that creates the flags / live events / campaigns /
  experiments behind each recipe in one step.
- LiveOps polish: flag value diff and payload sizes (`GET /admin/v1/flags/{key}/diff?a=&b=`,
  `GET /admin/v1/flags/{key}/sizes`, "Diff & size" tab in the console), `GET /v1/flags/overrides`
  (every identity / experiment / live-event override applied to the caller), deterministic
  precedence when several running experiments pin one flag (`priority`, then earliest start — all
  of them still enrol), participation duration counted per live-event run, rich template preview
  (HTML, pretty-printed JSON, byte size), direct messages that skip the inbox, resolve a template
  by key or display name, or go out through integration channels without a template, integration
  health (last status / error / time, sent and failed counters) in the API and the console,
  rejected events exported to data sinks as `$event_error` (the ingest response now reports
  `rejected`), `$identity_delete` emitted on GDPR deletion and kept as a tombstone, event body
  limits, player event search (`name`, `q`, `from`, `to` on
  `GET /admin/v1/identities/{id}/events`), per-key config export / import (`keys=`), `409 Conflict`
  when deleting a flag, audience, template, integration, metric, experiment or live event that
  something still references (the referrers are listed), and list filters that persist per page in
  the console. gRPC `SendEvents` now runs the same taxonomy validation as `POST /v1/events`.
- Release engineering: tag-driven release workflow (multi-arch GHCR images for the server and the
  Nakama bundle, binaries for Linux/macOS/Windows, SBOMs, provenance attestations, keyless cosign
  signatures, signed checksums, SDK archives), `security.yml` (cargo-deny, cargo-audit, npm audit,
  govulncheck, trivy), `deny.toml`, npm publishing for the TypeScript SDK, build provenance in
  `zanshin --version`, `/` and `GET /admin/v1/system`.
- Deployment kit (`deploy/`): hardened single-host Compose stack (Caddy TLS, two replicas, file
  secrets, internal network, nightly PostgreSQL + ClickHouse backups with restore scripts, optional
  Prometheus + Grafana profile), Helm chart (migration hook, probes, HPA, PDB, topology spread,
  allow-listed admin Ingress, ServiceMonitor + PrometheusRule, NetworkPolicy), Grafana overview
  dashboard and alert rules, k6 smoke / baseline / stream load tests, `docs/production.md`
  (topology, sizing, upgrades, backup/restore, DR, observability, security checklist).
- `zanshin_job_last_run_seconds` gauge per housekeeping job (feeds the stalled-housekeeping alert).
- Console users and fine-grained permissions: email + password login (Argon2id) with TOTP second
  factor and single-use recovery codes, server-side sessions (list / revoke, sign out everywhere),
  admin-issued password-reset links, "create first admin" bootstrap flow; `resource:action`
  grants (`read` / `write` / `delete` over 18 resources, `*` wildcard with per-resource overrides)
  enforced on every `/admin/v1` route with a `permission required: flags:write` hint, archetype
  presets (Admin, Developer, Game Designer, Debugger, Support, Analyst, Viewer), permission sets
  on admin API keys, key rotation with a grace period (`POST /admin/v1/api-keys/{id}/rotate`),
  `GET /admin/v1/permissions` catalog, Users and Account pages in the console with a permission
  matrix editor, navigation and controls that follow the caller's grants.
||||||| constructed merge base
- Analytics v3: events carry `country` / `platform` / `app_version` (denormalised from identity
  defaults at ingest) and every event-based report accepts a dimension filter (`country`,
  `platform`, `app_version`, `experiment` + `variant`); LiveOps **Status** board
  (`GET /admin/v1/analytics/status`: online now, DAU today, events/min, running experiments /
  live events / campaigns / journeys, upcoming live events, 24 h errors) and **Calendar**
  (`GET /admin/v1/analytics/calendar`: live-event occurrences, experiments, cron campaign fire
  windows, annotations, DAU overlay; 1 / 2 / 4-week views); **Explore** (`POST
  /admin/v1/analytics/explore`: any event bucketed by day/hour, grouped by country / platform /
  version / experiment variant / metadata field, with sums); **player map** (`/analytics/map`);
  **playtime** (`/analytics/playtime`: sessions, avg / median length, playtime per player-day,
  histogram); funnel models `unlimited` / `window` / `step` / `session`; cumulative retention +
  install counts; **LTV / RoAS / CPI** per install cohort (`/analytics/ltv`) with advertising
  spend import (`/analytics/ad-spend`, JSON or CSV); **errors** summary (`/analytics/errors`:
  rejected events by reason, failed message / webhook / export deliveries). Dashboards honour
  their `filters` as an event filter; funnel widgets accept `model`, retention widgets `cumulative`.

### Security
- rustls 0.23.46 (RUSTSEC-2026-0346..0349), maxminddb 0.27 (RUSTSEC-2025-0132).

### Fixed
- Console list pages backed by `{ items: [...] }` endpoints (templates, integrations, metrics, journeys, webhooks, export sinks) rendered a blank page; detail pages wrapped as `{ <kind>: ... }` loaded empty forms.

## [pre-release] — development history

Initial public release line. Everything below shipped on `main` while the API settled.

### Core
- Identities, sessions (JWT access/refresh), API keys (client/server/admin, read-only, expiring,
  quotas), identify/merge of anonymous identities, default/custom/computed/read-only properties,
  geo-IP defaults, play sessions and synthetic `$…` events, taxonomy with event definitions,
  JSON Schema validation and a rejected-events debugger, labels.
- Feature flags with typed values, rule builder, audiences, percentage rollouts, weighted variants,
  per-identity overrides, override chain (identity → experiment → live event → rules → default)
  with explain, value schemas.
- Audiences: rule-based and expression language, stored membership with scheduled recompute,
  includes/excludes, managed audiences, usage references, defaults.
- Experiments: variants, allocation, split keys, phases, caps/deadlines/locks, metrics catalog
  (goal/monitor), per-variant results with lift vs control, participants, CSV export.
- Live events: one-shot and cron windows, explicit join, sticky participation, per-player
  duration, in-event flag overrides, timeline, QA time travel.
- Messaging: inbox, campaigns, templates with translations, schedules (time and live-event
  relative), delivery integrations (webhook, OneSignal, Facebook, FCM, APNs), push tokens.
- Webhooks with HMAC signatures, batching and retries; journeys with triggers, steps, waits and
  branches; analytics (DAU/WAU/MAU, revenue, retention, funnels, annotations, dashboards) on
  ClickHouse or PostgreSQL; export sinks (S3-compatible, HTTP, BigQuery), configuration bundles
  with diff/import/history/undo, GDPR takeout and deletion, sample data.
- Hardening: Redis token-bucket rate limiting, audit log, security headers, CORS, OIDC SSO with
  viewer/editor/admin roles.
- Production foundation: cluster-wide job locks, OpenTelemetry tracing, per-route HTTP and gRPC
  metrics, gRPC limits, file-backed secrets, idempotent event ingest, configurable retention,
  deep readiness, two-phase graceful shutdown, `--migrate`.

### SDKs
- Go, TypeScript, Unity (C#), Unreal (C++), Godot (GDScript) client SDKs and the Nakama Go
  runtime module.

### Console
- Embedded React admin console covering every subsystem: grouped navigation, command palette,
  filterable lists, detail tabs with history and activity, creation wizards, dark theme.
