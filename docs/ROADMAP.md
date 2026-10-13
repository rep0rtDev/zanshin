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
- [ ] **Messages / inbox**: scheduled or triggered in-app messages, per-identity read /
      consumed state, templating with identity properties
- [x] **Analytics**: `POST /v1/events` (client) and `/server/v1/events` (trusted), batched writer
      into ClickHouse or PostgreSQL, events tagged with the identity's live experiment
      assignments, admin timeseries / top names / per-identity log, experiment results by metric
      (count, unique identities, sum, conversion per variant)
- [ ] **Identify / merge**: link an anonymous identity to an authenticated id, merge
      properties and history
- [x] **Go SDK** (`sdk/go`): server-key client, per-identity evaluation cache with TTL,
      WebSocket change-stream invalidation with reconnect/backoff, identity upsert, events,
      direct messages, inbox

## 0.2 — reach

- TypeScript SDK (browser + Node), Unity (C#), Unreal (C++), Godot (GDScript/C#) SDKs
  generated from `proto/`
- Push notifications (FCM / APNs) as a message delivery channel
- Webhooks and outbound integrations (Slack / Discord / generic)
- Multi-project (tenant) support with per-project keys
- Rate limiting, audit log of admin actions

## 0.3 — console

- Embedded React admin console served by the binary: flags, audiences, experiments with
  live results, live events calendar, message composer, identity explorer, dashboards
- Role-based admin accounts (OIDC) in addition to API keys
