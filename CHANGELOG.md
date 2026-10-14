# Changelog

All notable changes to Zanshin are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [SemVer](https://semver.org).
Release automation reads the section whose heading contains the tagged version.

## [Unreleased]

### Added
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

## [0.1.0] - Unreleased

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
