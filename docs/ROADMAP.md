# Roadmap

Zanshin is built core-first: the server API lands module by module, SDKs follow the API, the admin
console comes last once the model is stable.

## 0.1 — core (in progress)

- [x] Workspace, configuration, PostgreSQL / Redis / ClickHouse wiring, migrations
- [x] Identities, sessions, API keys, REST + gRPC, observability
- [ ] **Feature flags**: typed values (bool / string / number / JSON), targeting rules over
      properties and audiences, percentage rollouts with deterministic hashing, variants,
      Redis-backed cache with pub/sub invalidation, WebSocket / server-stream flag updates
- [ ] **Audiences**: rule-based segments (property predicates, and/or/not, set membership,
      ranges, version comparisons), membership evaluation on read, materialised counts
- [ ] **Experiments**: variants with weights, allocation percentage, audience gate, sticky
      assignment stored per identity, exposure events, start/stop/pause
- [ ] **Live events**: start/end, recurring schedules (cron + duration), audience gate,
      per-identity active list, per-event JSON payload, timezone handling
- [ ] **Messages / inbox**: scheduled or triggered in-app messages, per-identity read /
      consumed state, templating with identity properties
- [ ] **Analytics**: batched event ingest, ClickHouse sink with PostgreSQL fallback,
      event→property rules (computed properties), basic aggregations API
- [ ] **Identify / merge**: link an anonymous identity to an authenticated id, merge
      properties and history
- [ ] **Go SDK** (`sdk/go`) with local evaluation cache + **Nakama runtime module**
      (`bridges/nakama`): RPCs for every client call, hooks after-authenticate /
      session-start to sync properties, flag lookup without network round-trips

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
