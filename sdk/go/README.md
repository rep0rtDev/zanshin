# zanshin-go

Go client for the Zanshin server-key API, for game backends and Nakama runtime modules.

```go
z := zanshin.New("http://zanshin:7550", os.Getenv("ZANSHIN_SERVER_KEY"))
z.StartStream(ctx) // invalidate the local cache on every configuration change
defer z.Close()

z.UpsertIdentity(ctx, userID, map[string]any{"platform": "android"}, nil)
f, _ := z.Flag(ctx, userID, "shop_v2")
if f.Bool() { /* … */ }
exps, _ := z.Experiments(ctx, userID)
z.Track(ctx, userID, zanshin.Event{Name: "purchase", Value: "9.99"})
```

Per-identity evaluations (`Flags`, `Audiences`, `Experiments`, `LiveEvents`) are cached for
`WithCacheTTL` (default 30 s) and dropped as soon as the change stream reports any change, so a
flag flipped in the admin API reaches every backend within a round-trip.

## Server-side identity and participation helpers

```go
z.Identify(ctx, deviceID, accountID)                 // link / merge identities (e.g. after account sign-in)
z.AddAudienceMembers(ctx, "vip", accountID)          // managed audiences (guild rosters, support lists…)
z.RemoveAudienceMembers(ctx, "vip", accountID)
z.ExplainFlag(ctx, accountID, "shop_v2")             // override chain for support tooling
z.JoinLiveEvent(ctx, accountID, "weekend_boost")     // explicit live-event participation
z.LeaveLiveEvent(ctx, accountID, "weekend_boost")
z.MarkRead(ctx, accountID, msgID); z.Consume(ctx, accountID, msgID); z.DeleteInboxMessage(ctx, accountID, msgID)
```

## Local evaluation and OpenFeature

```go
z := zanshin.New(url, serverKey, zanshin.WithLocalEvaluation(time.Minute))
z.StartStream(ctx) // optional: refresh the rules bundle as soon as a flag changes
flags, err := z.LocalFlags(ctx, "player-42", map[string]map[string]any{"custom": {"level": 7}})
```

`LocalFlags` / `LocalFlag` evaluate in-process from `GET /server/v1/rules` with the server's exact
semantics (same hashing, operators and reasons) — no network call per player. Overrides, experiments
and live events are not applied locally; managed audiences never match (`Evaluator.DependsOnManaged`).

```go
import zof "github.com/rep0rtDev/zanshin/sdk/go/openfeature"
openfeature.SetProvider(zof.New(z, zof.Local()))
on, _ := openfeature.NewClient("game").BooleanValue(ctx, "shop", false,
    openfeature.NewEvaluationContext("player-42", map[string]any{"level": 7}))
```
