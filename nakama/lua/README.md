# Zanshin for the Nakama Lua runtime

Same RPC surface and identity sync as the Go plugin and the TypeScript module, as a single Lua
file for Nakama's Lua runtime. Talks to Zanshin's server API (`/server/v1`) over `nk.http_request`.

```sh
cp zanshin.lua /nakama/data/modules/
```

```yaml
runtime:
  env:
    - "ZANSHIN_URL=http://zanshin:7550"
    - "ZANSHIN_SERVER_KEY=zk_s_..."
```

RPCs: `zanshin.flags`, `zanshin.flag`, `zanshin.flag_explain`, `zanshin.audiences`,
`zanshin.experiments`, `zanshin.expose`, `zanshin.live_events`, `zanshin.live_event_join`,
`zanshin.live_event_leave`, `zanshin.inbox`, `zanshin.inbox_read`, `zanshin.inbox_delete`,
`zanshin.track`, `zanshin.properties`, `zanshin.audience_members` (server-to-server only).
