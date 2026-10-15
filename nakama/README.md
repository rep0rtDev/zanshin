# Zanshin module for Nakama

Go runtime plugin that connects a Nakama server to Zanshin.

- On every successful authenticate (device / email / custom / Google / Apple / Steam) the Nakama
  user id is upserted as a Zanshin identity with `default` properties `auth_provider`, `username`,
  `lang`, `location`, `timezone`, `client_ip`, `created_at` — all targetable in rules.
- RPCs for clients (user id taken from the session):

| RPC | payload | response |
|---|---|---|
| `zanshin.flags` | `{"names":["shop_v2"]}` (optional) | `{"flags":[…]}` |
| `zanshin.audiences` | — | `{"audiences":[…]}` |
| `zanshin.experiments` | `{"names":[…]}` | `{"experiments":[…]}` (sticky assignment) |
| `zanshin.expose` | `{"experiment":"price_test"}` | `{}` |
| `zanshin.live_events` | `{"upcoming":true,"names":[…]}` | `{"live_events":[…]}` |
| `zanshin.inbox` | `{"limit":50}` | `{"messages":[…]}` |
| `zanshin.track` | `{"events":[{"name":"purchase","value":"9.99"}]}` | `{"accepted":n}` |
| `zanshin.properties` | `{"custom":{"nickname":"Neo"}}` | properties |
| `zanshin.flag` | `{"name":"shop_v2"}` | a single evaluation |
| `zanshin.flag_explain` | `{"name":"shop_v2"}` | override chain (identity → experiment → live event → rules → default) |
| `zanshin.live_event_join` | `{"key":"weekend_boost"}` | the live event as the player now sees it |
| `zanshin.live_event_leave` | `{"key":"weekend_boost"}` | `{"left":true}` |
| `zanshin.inbox_read` | `{"id":"…"}` or `{"id":"…","consume":true}` | the message, or `{"consumed":id}` |
| `zanshin.inbox_delete` | `{"id":"…"}` | `{"deleted":id}` |
| `zanshin.audience_members` | `{"audience":"vip","add":[…],"remove":[…]}` — server-to-server only (HTTP key) | `{"added":n,"removed":n}` |

Evaluations are cached in-process (`ZANSHIN_CACHE_TTL`, default 30 s) and invalidated instantly
through the server change stream, so Nakama answers most flag reads without a network hop.

Build (the plugin must match the Nakama Go toolchain, hence the pluginbuilder image):

```sh
docker build -f nakama/Dockerfile --build-arg NAKAMA_VERSION=3.22.0 -t nakama-zanshin .
docker run -e ZANSHIN_URL=http://zanshin:7550 -e ZANSHIN_SERVER_KEY=… nakama-zanshin
```

## Zero-code core events

The module mirrors Nakama's own server-side actions into Zanshin's core event catalog, without any
client work: validated purchases (Apple / Google / Huawei / Facebook Instant → `purchaseCompleted`
with `store`, `product_id`, `transaction_id`, `environment`), leaderboard and tournament record
writes (`eventLeaderboardScore` with `leaderboard_id`, `kind`, `score`, `subscore`, `rank`) and
friend invites (`inviteSent`). Events are sent in the background and never block the request.
Set `ZANSHIN_CORE_EVENTS=false` to turn the hooks off.
