# Zanshin .NET SDK (server-side)

Two NuGet packages for .NET services — game servers, matchmakers, backend workers, anything that
holds a **server key**. For Unity clients use the UPM package in `sdk/unity`.

| Package | What |
| --- | --- |
| `Zanshin.Server` | Client for the server API (`/server/v1`): flags, experiments, audiences, live events, inbox, events and messages for any player by external id, with an in-memory cache invalidated by the change stream. `netstandard2.0` + `net8.0`, only `System.Text.Json`. |
| `Zanshin.OpenFeature` | [OpenFeature](https://openfeature.dev) provider on top of it. |

## Install

```sh
dotnet add package Zanshin.Server
dotnet add package Zanshin.OpenFeature   # optional
```

Until the packages are on nuget.org, reference the projects or the `.nupkg` files from a
[release](https://github.com/rep0rtDev/zanshin/releases) (`zanshin-dotnet-<version>.zip`).

## Quick start

```csharp
using Zanshin.Server;

var zanshin = new ZanshinClient("https://zanshin.example.com", Environment.GetEnvironmentVariable("ZANSHIN_SERVER_KEY")!);
zanshin.StartStream();                        // console edits drop the cache within a second

await zanshin.UpsertIdentityAsync("player-42", customProps: new Dictionary<string, object?> { ["tier"] = "gold" });

var flags = await zanshin.FlagsAsync("player-42");
bool doubleXp = await zanshin.FlagBoolAsync("player-42", "double_xp");
var price = await zanshin.FlagAsync("player-42", "shop_price");   // price.Value is a JsonElement; price.As<T>() for objects

foreach (var e in await zanshin.LiveEventsAsync("player-42")) Console.WriteLine($"{e.Key} active={e.Active}");
await zanshin.ExposeAsync("player-42", "pricing_test");
await zanshin.TrackAsync("player-42", new Event("match_end", "win", new Dictionary<string, object?> { ["map"] = "dust" }));
await zanshin.SendMessageAsync("player-42", new DirectMessage { Title = "Welcome back", Payload = new { gems = 50 } });
```

Reads are cached per player for `ZanshinOptions.CacheTtl` (30 s). `Invalidate()` /
`InvalidateIdentity(id)` drop entries by hand; writes through the client already do.
Errors surface as `ZanshinException` with `Status` and the API `Code`.

## OpenFeature

```csharp
using OpenFeature;
using Zanshin.OpenFeature;

await Api.Instance.SetProviderAsync(new ZanshinProvider(zanshin));
var client = Api.Instance.GetClient();
var ctx = EvaluationContext.Builder().SetTargetingKey("player-42").Build();
bool on = await client.GetBooleanValueAsync("double_xp", false, ctx);
```

The targeting key is the player's external id and is required. Evaluation happens on the server
against the identity's stored properties, so context attributes other than the targeting key are not
sent — push them with `UpdatePropertiesAsync` instead. Reasons map to OpenFeature's
(`rule`/`override`/`live_event` → `TARGETING_MATCH`, `variant`/`experiment` → `SPLIT`,
`disabled` → `DISABLED`, `default`/`prerequisite` → `DEFAULT`); `version`, `rule_id` and `experiment`
are exposed as flag metadata; a missing flag gives `FLAG_NOT_FOUND`, a wrong type `TYPE_MISMATCH`.

## Build

```sh
dotnet build Zanshin.Server -c Release && dotnet build Zanshin.OpenFeature -c Release
dotnet test Zanshin.Tests
dotnet pack Zanshin.Server -c Release -o dist && dotnet pack Zanshin.OpenFeature -c Release -o dist
```
