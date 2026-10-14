# Zanshin Unreal Engine SDK

Runtime plugin for Unreal Engine 5.3+ wrapping the Zanshin client API: sessions with automatic
refresh, feature flags, audiences, experiments + exposure, live events, inbox, push tokens, analytics
events and the live WebSocket stream. Depends only on engine modules (`HTTP`, `WebSockets`, `Json`).

## Install

Copy this folder to `<YourProject>/Plugins/Zanshin` (or add the repo as a submodule and symlink
`sdk/unreal`), then add `"Zanshin"` to your module's `PublicDependencyModuleNames` and enable the
plugin in `.uproject`.

## Blueprint / quick start — `UZanshinSubsystem`

```cpp
UZanshinSubsystem* Z = GetGameInstance()->GetSubsystem<UZanshinSubsystem>();
Z->Configure(TEXT("https://liveops.example.com"), TEXT("zk_c_..."));   // client API key
Z->Authenticate(PlayerId, {{TEXT("platform"), TEXT("windows")}}, {}, OnDone);
// later, synchronous reads from the streamed cache:
if (Z->GetFlagBool(TEXT("new_shop"))) { ... }
Z->Track(TEXT("level.up"), TEXT("12"), {});
Z->Expose(TEXT("onboarding_v2"));
```

`OnFlagChanged`, `OnSessionChanged` (persist both tokens, feed them to `Restore` next launch) and
`OnStreamStateChanged` are Blueprint-assignable dispatchers. Everything runs on the game thread.

## Full C++ API — `FZanshinClient`

```cpp
TSharedRef<FZanshinClient> Z = MakeShared<FZanshinClient>(Url, ClientKey);
Z->Authenticate(Id, DefaultProps, CustomProps, [](const TZanshinResult<FZanshinSession>& R) {
    if (!R.bOk) { UE_LOG(LogTemp, Warning, TEXT("%s"), *R.Error.ToString()); return; }
});
Z->FlagNumber(TEXT("daily_reward_coins"), 100.0, [](double Coins) { ... });
Z->GetExperiments({}, [](const TZanshinResult<TArray<FZanshinExperiment>>& R) { ... });
Z->GetLiveEvents({}, /*bUpcoming*/ false, ...);
Z->GetInbox(20, ...);  Z->MarkRead(Id, ...);  Z->Consume(Id, ...);
Z->DeleteInboxMessage(Id, ...);  Z->JoinLiveEvent(TEXT("weekend_boost"), ...);  Z->LeaveLiveEvent(TEXT("weekend_boost"), ...);
Z->Identify(TEXT("account-42"), ...);   // anonymous → account, merges if it exists, switches session
Z->RegisterPushToken(TEXT("fcm"), Token, DeviceId, ...);
Z->Track({ Event1, Event2 }, ...);

TSharedRef<FZanshinStream> S = Z->Stream({ TEXT("new_shop"), TEXT("daily_reward_coins") });
S->OnFlagChanged.AddLambda([](const FZanshinFlag& F) { ... });
S->OnInboxChanged.AddLambda([](const TArray<FZanshinInboxMessage>& Inbox) { ... });
S->Connect();
```

- Every call takes a completion `TFunction` receiving `TZanshinResult<T>` (`bOk`, `Value`, `Error`
  with `Status` / `Code` / `Message`; `IsNotFound()`, `IsUnauthenticated()`, `IsRateLimited()`).
- Session calls refresh the access token automatically when it is within `RefreshLeewaySeconds`
  of expiry; concurrent callers share one refresh. A 401 clears the local tokens and fires
  `OnSessionChanged` with an empty session.
- `FZanshinFlag::AsBool/AsString/AsNumber/AsObject` coerce the JSON value with a fallback.
- `FZanshinStream` reconnects with jittered exponential backoff (≤30 s), pings every 30 s,
  refreshes the session on an auth close and keeps `GetFlags()` current from pushes.

## Notes

- Request bodies and responses are plain `FJsonObject`s, so any server field is reachable even if
  it is not mirrored in the typed structs.
- The plugin has no editor module and no content; it compiles for every platform with `HTTP` and
  `WebSockets` support (desktop, consoles, mobile). HTML5 is not supported by Unreal's WebSockets.
