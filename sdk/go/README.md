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
