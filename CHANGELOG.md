# Changelog

All notable changes to Zanshin are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [SemVer](https://semver.org).
Release automation reads the section whose heading contains the tagged version.

## [Unreleased]

### Added
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

### Security
- rustls 0.23.46 (RUSTSEC-2026-0346..0349), maxminddb 0.27 (RUSTSEC-2025-0132).

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
