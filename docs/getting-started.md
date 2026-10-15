# Getting started

Ten minutes from zero to a flag evaluated for a real player.

## 1. Run the stack

```sh
git clone https://github.com/rep0rtDev/zanshin && cd zanshin
docker compose up -d                 # PostgreSQL, Redis, ClickHouse, Zanshin on :7350
docker compose --profile demo up demo  # optional: sample config + 500 synthetic players with 30 days of events
```

Open the console at <http://localhost:7350>. The first login uses the bootstrap key from
`docker-compose.yml` (`ZANSHIN__ADMIN__BOOTSTRAP_KEY`); create your own user right away under
*Settings → Users* and remove the bootstrap key from production configs (`zanshin --doctor` reminds you).

## 2. Create a server key and a flag

Console → *Platform → API keys → New* (kind `server`), or with the CLI:

```sh
zanshinctl --url http://localhost:7350 --key <bootstrap-key> keys create --kind server --name nakama
zanshinctl flags set shop_v2 --value-type bool --default-value false
```

## 3. Connect your game backend

=== "Nakama (zero code)"

    Use the `ghcr.io/rep0rtdev/zanshin-nakama` image or drop `zanshin.so` into your modules folder, then:

    ```yaml
    environment:
      ZANSHIN_URL: http://zanshin:7350
      ZANSHIN_SERVER_KEY: <server key>
    ```

    Every authenticated Nakama user becomes a Zanshin identity; purchases, leaderboard writes and
    friend invites flow in as core events; clients call `zanshin.flags`, `zanshin.live_events`,
    `zanshin.track` RPCs.

=== "Any backend (SDKs)"

    Go, TypeScript, C#, C++, Swift, Kotlin, Dart, Unity, Unreal, Godot, Defold — see
    [API reference](api.md). All server SDKs can also evaluate flags locally from a rule bundle.

## 4. Evaluate for a player

```sh
curl -s -H "x-api-key: <server key>" \
  "http://localhost:7350/server/v1/flags?external_id=player-1&names=shop_v2"
```

## 5. Next steps

* Audiences and targeting rules → *Audiences*, then a flag rule per audience.
* Experiments with sequential testing, guardrails and bandits → *Experiments*.
* Live events, journeys, messaging → the LiveOps section of the console.
* `zanshinctl demo reset` removes the demo data again (`docker compose run --rm demo demo reset`).
* The *Getting started* card on the Overview page tracks these steps against your real data.
* Going to production: [Production guide](production.md) (sizing, TLS, backups, `zanshin --doctor`).
