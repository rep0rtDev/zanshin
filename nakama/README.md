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

Evaluations are cached in-process (`ZANSHIN_CACHE_TTL`, default 30 s) and invalidated instantly
through the server change stream, so Nakama answers most flag reads without a network hop.

Build (the plugin must match the Nakama Go toolchain, hence the pluginbuilder image):

```sh
docker build -f nakama/Dockerfile --build-arg NAKAMA_VERSION=3.22.0 -t nakama-zanshin .
docker run -e ZANSHIN_URL=http://zanshin:7550 -e ZANSHIN_SERVER_KEY=… nakama-zanshin
```
