# Zanshin Swift SDK

Swift Package for iOS 15+ / macOS 12+ / tvOS / watchOS / visionOS and Linux. Zero dependencies
(Foundation `URLSession` + `URLSessionWebSocketTask`), `async`/`await`, actors.

```swift
dependencies: [.package(url: "https://github.com/rep0rtDev/zanshin.git", branch: "main")],
targets: [.target(name: "Game", dependencies: [.product(name: "Zanshin", package: "zanshin")])]
```
(The package lives in `sdk/swift`; point a local or mirrored package at that directory.)

```swift
import Zanshin

let z = ZanshinClient(url: URL(string: "https://liveops.example.com")!, apiKey: "zk_c_...",
                      onSession: { tokens in persist(tokens) })
let session = try await z.authenticate(id: deviceId, default: ["platform": "ios", "app_version": "1.4.0"])

let shop = try await z.flag("shop_v2").value.boolValue ?? false
for e in try await z.experiments() { print(e.key, e.variant) }
try await z.expose("price_test")
let events = try await z.liveEvents()
let inbox = try await z.inbox()
try await z.event("level_complete", value: "12", metadata: ["stars": 3])

let stream = ZanshinStream(client: z)
await stream.onFlag { flag in apply(flag) }
await stream.connect()
```

Surface: `authenticate` / `restore` / `refresh` / `logout`, `identity` / `deleteIdentity`,
`properties` / `updateProperties`, `identify`, `flags` / `flag` / `flagValue` / `flagOverrides`,
`audiences`, `experiments` / `expose`, `liveEvents` / `joinLiveEvent` / `leaveLiveEvent`,
`inbox` / `markRead` / `consume` / `deleteInboxMessage`, push tokens, `track` / `event`, and
`ZanshinStream` (flag cache, ping, reconnect with backoff, re-auth after refresh).

`swift build && swift test`.
