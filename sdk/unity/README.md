# Zanshin Unity SDK

UPM package, no dependencies, Unity 2021.3+ (netstandard2.1). Add via Package Manager →
"Add package from git URL": `https://github.com/rep0rtDev/zanshin.git?path=sdk/unity`.

```csharp
using Zanshin;

var z = new ZanshinClient("https://liveops.example.com", CLIENT_KEY);
z.SessionChanged += s => { if (s != null) Save(s.Token, s.RefreshToken); else Forget(); };

await z.Authenticate(SystemInfo.deviceUniqueIdentifier,
    new Dictionary<string, object> { ["platform"] = Application.platform.ToString(), ["version"] = Application.version });
// or: await z.Restore(savedToken, savedRefreshToken);

bool shopV2 = await z.FlagBool("shop_v2");
var exps = await z.Experiments();           // sticky variants
await z.Expose("price_test");
var live = await z.LiveEvents();            // active now
var inbox = await z.Inbox();
await z.RegisterPushToken("fcm", fcmToken, SystemInfo.deviceUniqueIdentifier);
await z.Track("purchase", 9.99, new Dictionary<string, object> { ["sku"] = "gold" });

var stream = z.Stream();                    // live updates (not on WebGL)
stream.FlagChanged += f => mainThread.Enqueue(() => Apply(f));
stream.Connect();
```

Access tokens refresh automatically (one shared refresh for concurrent calls); the stream
reconnects with backoff. Stream events fire on a background thread — marshal to the main
thread. JSON values are plain `Dictionary<string, object>` / `List<object>` / `double` / `long` /
`string` / `bool` (see `Json`).
