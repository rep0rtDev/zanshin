# Roadmap

Zanshin was built core-first: the server API landed module by module, SDKs followed the API, the
admin console came once the model was stable, then two "gap rounds" against Satori closed the
remaining feature and production distance. Everything below is checked against `main`.

## 0.1 — core ✅

- [x] Workspace, configuration, PostgreSQL / Redis / ClickHouse wiring, embedded migrations
- [x] Identities, sessions (JWT access + rotating refresh, replay detection), API keys, REST + gRPC,
      observability (Prometheus, structured logs, request ids, graceful shutdown)
- [x] **Feature flags**: typed values, targeting rules over properties, percentage rollouts with
      deterministic hashing, weighted variants, per-identity overrides, in-memory catalog replicated
      through Redis pub/sub, WebSocket / gRPC server-stream updates
- [x] **Audiences**: rule-based segments with sampling, gating flag rules and experiments
- [x] **Experiments**: variants, allocation, audience gate, sticky assignment, exposures, lifecycle,
      per-variant results, flag linkage
- [x] **Live events**: one-shot windows and cron recurrence, targeting, payload, current / next
      occurrence, live push
- [x] **Messaging**: inbox, campaigns, direct messages, push notifications (FCM HTTP v1 / APNs, token
      registry, proactive sweep)
- [x] **Analytics**: client / server ingest, batched writer into ClickHouse or PostgreSQL, computed
      properties, experiment metrics
- [x] **Identify / merge**, hardening (Redis token-bucket rate limiting, audit log, read-only / expiring
      keys, security headers / CSP / HSTS / trust_proxy)

## 0.2 — SDKs and console ✅

- [x] Go (`sdk/go`), TypeScript (`sdk/ts`), Unity (`sdk/unity`), Unreal (`sdk/unreal`), Godot
      (`sdk/godot`) SDKs; Nakama Go runtime module (`nakama/`)
- [x] Embedded React console with a screen for every admin surface; console v2 (visual rule builder,
      calendar, template preview, comparative charts, audit log, dark theme)
- [x] Console SSO through OIDC with roles `viewer` / `editor` / `admin`

## 0.3 — LiveOps parity (gap round 1) ✅

- [x] **Taxonomy & player model**: labels, event definitions with value types and metadata JSON
      Schema (`off` / `warn` / `strict`), auto-discovery, rejected-event debugger, read-only custom
      properties, value schemas, default-property enrichment (headers / geo-IP), `POST /v1/identify`,
      play sessions and synthetic lifecycle events
- [x] **Audiences v2**: stored membership (live mirror / scheduled recompute), member lists,
      include / exclude lists with expiring exclusions, managed audiences, usage references, copy,
      expression language, `within_last` / `older_than`, default audiences
- [x] **Flags v2 / live events v2**: audience-targeted variants with priority, override chain with
      explain, explicit join / sticky membership, participation duration, per-event flag overrides,
      timeline, QA future-date mode
- [x] **Metrics + experiments v2**: metrics catalog, per-variant results with lift, copy / phases,
      lock, interrupt with winner → flag, participant caps, enrolment deadline, split key, series,
      participants list and CSV
- [x] **Messaging v2**: templates (plaintext / HTML / JSON, translations, test send), schedules (cron
      and live-event relative), integrations (webhook HMAC, OneSignal, Facebook, FCM / APNs),
      per-channel delivery log, inbox delete
- [x] **Webhooks**: event-type patterns, HMAC, batching, retries with backoff, delivery log, test send
- [x] **Journeys**: triggers, steps (message, track, flag override, experiment assign, integration,
      wait, local-time wait, branch, goto, exit), cooldown / caps, persisted runs, step editor
- [x] **Analytics v2**: game-metrics overview, retention cohorts, funnels, annotations, custom
      dashboards
- [x] **Data management**: export sinks (S3-compatible / HTTP / BigQuery), configuration bundles
      (export, diff, merge / replace import, import from another server), change history with revert,
      audit undo, GDPR takeout / delete, event purge, sample data
- [x] **Console redesign** (shell, lists, detail tabs, wizards, command palette) and **SDK / Nakama
      parity** for every new API

## 0.4 — production and gap round 2 ✅

- [x] **Production foundation**: cluster-wide job locks for housekeeping, OpenTelemetry tracing,
      per-route HTTP / gRPC / pool metrics, gRPC rate limits and timeouts, `ZANSHIN_FILE__` secrets,
      `server.production` guard rails, event idempotency, configurable retention, deep `/readyz`,
      two-phase drain, batched event writer, HTTP load shedding
- [x] **Release engineering**: tag-driven releases (multi-arch GHCR images for server and Nakama
      bundle, binaries, SBOM, provenance, cosign, signed checksums, SDK archives), `security.yml`
      (cargo-deny / cargo-audit / npm audit / govulncheck / trivy), build info in `--version` and
      `/admin/v1/system`
- [x] **Deploy kit**: production Compose (Caddy TLS, replicas, file secrets, backups + restore), Helm
      chart (migration hook, probes, HPA / PDB, ServiceMonitor / PrometheusRule), Grafana dashboard
      and alerts, k6 load tests, `docs/production.md`
- [x] **Console users + fine-grained permissions**: email + password (Argon2id), TOTP MFA with
      recovery codes, server-side sessions, `resource:action` permissions with presets, permission
      sets and rotation for admin keys, permission hints in the console
- [x] **Analytics v3**: status board, LiveOps calendar, country / platform / version / variant filters,
      explorer, player map, playtime, funnel models, cumulative retention, LTV / CPI / RoAS with ad
      spend import, errors widget
- [x] **LiveOps polish**: flag diff and payload sizes, `GET /v1/flags/overrides`, experiment
      precedence on a shared flag, participation per run, rich previews, skip-inbox / template-less
      direct messages, integration health, `$event_error` / `$identity_delete`, event size limits,
      player event search, per-key export / import, `409` on referenced delete, persistent filters
- [x] **SDK wave 2**: gRPC parity with REST, Nakama JavaScript (`nakama/ts`) and Lua (`nakama/lua`)
      runtime modules, Swift (`sdk/swift`), Kotlin / JVM (`sdk/kotlin`), Dart / Flutter (`sdk/dart`)
      SDKs, LiveOps recipes library in the console

## Known gaps / not verified end to end

- Unreal plugin is not compiled in CI (no engine toolchain); Swift / Kotlin / Dart are built on Linux
  hosts, not on devices.
- FCM / APNs / OneSignal / Facebook / BigQuery providers are exercised against local fakes only.
- OIDC and SAML are verified end to end against Keycloak in CI (`live` job); SCIM against replayed Okta / Entra
  request shapes — not yet against a live Okta / Entra tenant.
