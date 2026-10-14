# Zanshin for the Nakama JavaScript runtime

Same RPC surface and identity sync as the Go plugin (`nakama/main.go`), for deployments that run
Nakama's JavaScript runtime instead of Go plugins. No dependencies: the module talks to Zanshin's
server API (`/server/v1`) over `nk.httpRequest` with a server key.

```sh
npm ci && npm run build          # → build/zanshin.js (single ES5 file)
cp build/zanshin.js /nakama/data/modules/
```

Nakama config:

```yaml
runtime:
  js_entrypoint: zanshin.js
  env:
    - "ZANSHIN_URL=http://zanshin:7550"
    - "ZANSHIN_SERVER_KEY=zk_s_..."
```

RPCs (payload → response, identical to the Go module): `zanshin.flags`, `zanshin.flag`,
`zanshin.flag_explain`, `zanshin.audiences`, `zanshin.experiments`, `zanshin.expose`,
`zanshin.live_events`, `zanshin.live_event_join`, `zanshin.live_event_leave`, `zanshin.inbox`,
`zanshin.inbox_read`, `zanshin.inbox_delete`, `zanshin.track`, `zanshin.properties`,
`zanshin.audience_members` (server-to-server only). Users are upserted as Zanshin identities after
every authenticate (device / email / custom / Google / Apple / Steam).
