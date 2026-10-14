# Zanshin Dart / Flutter SDK

Pure Dart (`dart:io` `HttpClient` + `WebSocket`), zero dependencies, works in Flutter (mobile /
desktop) and server-side Dart.

```yaml
dependencies:
  zanshin:
    git:
      url: https://github.com/rep0rtDev/zanshin.git
      path: sdk/dart
```

```dart
final z = ZanshinClient(url: 'https://liveops.example.com', apiKey: 'zk_c_...',
    onSession: (t) => persist(t));
await z.authenticate(deviceId, defaults: {'platform': 'android', 'app_version': '1.4.0'});

final shop = (await z.flag('shop_v2')).boolValue ?? false;
for (final e in await z.experiments()) print('${e.key} -> ${e.variant}');
await z.expose('price_test');
final inbox = await z.inbox();
await z.event('level_complete', value: 12, metadata: {'stars': 3});

final stream = ZanshinStream(z);
stream.flagChanges.listen(apply);
stream.connect();
```

Surface: `authenticate` / `restore` / `refresh` / `logout`, `identity` / `deleteIdentity`,
`properties` / `updateProperties`, `identify`, `flags` / `flag` / `flagValue` / `flagOverrides`,
`audiences`, `experiments` / `expose`, `liveEvents` / `joinLiveEvent` / `leaveLiveEvent`,
`inbox` / `markRead` / `consume` / `deleteInboxMessage`, push tokens, `track` / `event`, and
`ZanshinStream` (flag cache, ping, reconnect with backoff, re-auth after refresh).

`dart pub get && dart analyze && dart test`.
