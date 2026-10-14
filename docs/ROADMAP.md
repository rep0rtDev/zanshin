# Roadmap

Zanshin is built core-first: the server API lands module by module, SDKs follow the API, the admin
console comes last once the model is stable.

## 0.1 — core (in progress)

- [x] Workspace, configuration, PostgreSQL / Redis / ClickHouse wiring, migrations
- [x] Identities, sessions, API keys, REST + gRPC, observability
- [x] **Feature flags**: typed values (bool / string / number / JSON), targeting rules over
      properties (audience gate wired in the next step), percentage rollouts with deterministic
      hashing, weighted variants, per-identity overrides, in-memory catalog replicated through
      Redis pub/sub, WebSocket / gRPC server-stream flag updates
- [x] **Audiences**: rule-based segments (property predicates, and/or/not, set membership,
      ranges, version comparisons) with optional percentage sampling, evaluated on read, gating
      flag rules and experiments
- [x] **Experiments**: variants with weights, allocation percentage, audience gate, sticky
      assignment stored per identity, exposure tracking, draft → running → paused → stopped,
      per-variant results, optional link to a flag whose value the variant overrides
- [x] **Live events**: one-shot windows and cron recurrence (UTC) with per-occurrence duration,
      targeting by rules and audiences, free-form payload, current and next occurrence exposed to
      clients, live push on change
- [x] **Messaging v2**: templates (plaintext/HTML/JSON, per-locale translations, preview + test send), schedules
  (cron and live-event relative, once per occurrence), delivery integrations (webhook with HMAC, OneSignal, Facebook,
  FCM/APNs) with per-channel delivery log, inbox delete.
- [x] **Analytics**: `POST /v1/events` (client) and `/server/v1/events` (trusted), batched writer
      into ClickHouse or PostgreSQL, events tagged with the identity's live experiment
      assignments, admin timeseries / top names / per-identity log, experiment results by metric
      (count, unique identities, sum, conversion per variant)
- [x] **Identify / merge**: link an anonymous identity to an authenticated id, merge
      properties and history
- [x] **Go SDK** (`sdk/go`): server-key client, per-identity evaluation cache with TTL,
      WebSocket change-stream invalidation with reconnect/backoff, identity upsert, events,
      direct messages, inbox
- [x] **TypeScript SDK** (`sdk/ts`): zero-dependency client for browser/Node, session restore and
      single-flight refresh, all client endpoints, reconnecting WebSocket stream with local flag
      cache, integration test against a live server
- [x] **Push notifications**: `push_tokens` registry (`PUT/DELETE /v1/push-tokens`), FCM HTTP v1 and
      APNs providers behind a `Provider` trait (plus `log` provider for dev/test), campaigns and
      direct messages opt in with `push: true`, proactive sweep delivers live push campaigns to
      every registered device (every minute, after campaign changes, or `POST /admin/v1/push/sweep`),
      dead tokens pruned on provider feedback
- [x] **Admin console** (`console/`): React/Vite SPA embedded into the binary with `rust-embed` and served at
      `/console`; every admin API surface has a screen (flags with overrides + dry run, experiments with
      lifecycle + results/lift, audiences with membership check, live events, campaigns with stats,
      identities with properties/events/inbox/direct send/push devices, analytics, API keys)
- [x] **Console v2**: visual condition/rule builder (groups, 20 operators, typed values, weighted variants,
      audience chips), live-event month calendar backed by `GET /admin/v1/live-events/occurrences`, message
      template preview, comparative analytics chart with tooltips, audit log screen, API key scopes/quotas UI,
      dark theme
- [x] **Unreal SDK** (`sdk/unreal`): runtime plugin with `FZanshinClient` / `FZanshinStream` (engine HTTP,
      WebSockets, Json only) and a Blueprint `UZanshinSubsystem` with a synchronous streamed flag cache
- [x] **Godot SDK** (`sdk/godot`): GDScript addon with awaitable `Zanshin` autoload, typed results, live stream
      node; headless smoke test passes against the dev server, CI parses it with Godot 4.3
- [x] **Unity SDK** (`sdk/unity`): dependency-free UPM package (netstandard2.1, own JSON), async client
      with single-flight refresh, all client endpoints, reconnecting `ClientWebSocket` stream with
      local flag cache; compiled in CI with `dotnet build`, smoke-tested against a live server
- [x] **SDK wave 2**: gRPC parity with REST, Nakama JavaScript (`nakama/ts`) and Lua (`nakama/lua`) runtime
  modules, Swift (`sdk/swift`), Kotlin / JVM (`sdk/kotlin`) and Dart / Flutter (`sdk/dart`) SDKs, LiveOps
  recipes library in the console.

## 0.4 — LiveOps parity

- [x] **Taxonomy & player model**: labels registry (`/admin/v1/labels`, rename/delete propagate to every
  tagged resource, `?tags=` filters), event definitions with value types (`number` / `string` / `object` /
  `revenue`) and metadata JSON Schema, `off` / `warn` / `strict` enforcement, auto-discovery of unknown
  events, rejected-event debugger (`/admin/v1/taxonomy/rejected`), read-only custom properties,
  JSON Schemas for flag / live-event values (`/admin/v1/schemas`), default-property enrichment from
  `X-Zanshin-*` headers / `Accept-Language` / MaxMind geo-IP, `POST /v1/identify` (rename or merge),
  play sessions (`/admin/v1/identities/{id}/sessions`) with inactivity timeout, synthetic lifecycle events.
- [x] **Audiences v2**: stored membership (live mirror + scheduled recompute via cron / on demand), member
  lists with join time, include / exclude lists with expiring exclusions, managed audiences fed from the
  server API, usage references, copy, expression language (`docs/api.md`), `within_last` / `older_than`
  operators, default audiences.
- [ ] **Flags v2 / live events v2**: audience-targeted variants with priority, variant diff, override chain,
  explicit join / sticky membership, per-event flag overrides, timeline, QA future-date mode.
- [x] **Metrics + experiments v2**: metrics catalog (goal/monitor, count/unique/sum/avg/conversion), per-variant
  metric results with lift, copy / phases, lock, interrupt with winner → flag, participant caps + enrolment deadline,
  split key, enrolment + metric series, participants list and CSV.
- [ ] **Messaging v2**: reusable templates (plaintext / HTML / JSON, translations, test send), schedules
  (cron and live-event relative), per-channel delivery status, inbox deletion.
- [x] **Webhooks**: subscriptions with event-type patterns, HMAC-signed requests, batching, retries with backoff,
  persisted delivery log with manual retry, test send, console page.
- [x] **Journeys**: event / identity-created / audience-enter / live-event-start / manual triggers, steps (message,
  track, flag override, experiment assign, integration, wait, wait-until-local-time, branch, goto, exit), cooldown and
  entry caps, persisted runs with step log, operator exit, console step editor.
- [x] **Analytics v2**: game-metrics overview (DAU/WAU/MAU, new users, sessions, revenue, ARPDAU/ARPPU, paying share,
  stickiness), retention cohorts, funnels with conversion windows and step filters, chart annotations (automatic from
  configuration changes + notes), custom dashboards with widgets, Insights console page.
- [ ] **Data management**: export sinks (S3 / Parquet, BigQuery, HTTP), config export / import with diff,
  data deletion, sample data, audit undo.
- [x] **Console redesign** and **SDK / Nakama parity** for every new API.

## 0.2 — reach

- Webhooks and outbound integrations (Slack / Discord / generic)
- Multi-project (tenant) support with per-project keys
- [x] Rate limiting (Redis token bucket per scope/caller, per-key quotas), audit log of admin actions
      (`/admin/v1/audit`), read-only + expiring API keys, security headers / CSP / HSTS / trust_proxy

## 0.3 — console (done, see above)

- Embedded React admin console served by the binary: flags, audiences, experiments with
  live results, live events calendar, message composer, identity explorer, dashboards
- [x] Role-based admin accounts (OIDC) in addition to API keys: `admin.oidc` (code + PKCE, JWKS
      verification, claim → role map, domain allow-list), roles `viewer` / `editor` / `admin` on
      API keys and console sessions, `GET /admin/v1/me`, role-aware console (SSO button, API keys
      hidden from non-admins)
