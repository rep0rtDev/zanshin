# Zanshin Kotlin / JVM SDK

Kotlin library for Android (API 26+ with core library desugaring, or any JDK 11+ runtime) and
server-side JVM. Zero dependencies: JDK 11 `HttpClient` + `WebSocket` and a built-in JSON codec.
Blocking calls plus `...Async` twins returning `CompletableFuture`.

```kotlin
val z = ZanshinClient("https://liveops.example.com", "zk_c_...") { tokens -> persist(tokens) }
z.authenticate(deviceId, default = mapOf("platform" to "android", "app_version" to "1.4.0"))

val shop = z.flag("shop_v2").boolValue ?: false
z.experiments().forEach { println("${it.key} -> ${it.variant}") }
z.expose("price_test")
val inbox = z.inbox()
z.event("level_complete", 12, mapOf("stars" to 3))

val stream = ZanshinStream(z) { msg -> if (msg is StreamMessage.FlagChanged) apply(msg.flag) }
stream.connect()
```

Surface: `authenticate` / `restore` / `refresh` / `logout`, `identity` / `deleteIdentity`,
`properties` / `updateProperties`, `identify`, `flags` / `flag` / `flagValue` / `flagOverrides`,
`audiences`, `experiments` / `expose`, `liveEvents` / `joinLiveEvent` / `leaveLiveEvent`,
`inbox` / `markRead` / `consume` / `deleteInboxMessage`, push tokens, `track` / `event`, and
`ZanshinStream` (flag cache, ping, reconnect with backoff, re-auth after refresh).

Build: `gradle build` (Kotlin 2.0, JVM toolchain 11). Coroutines: wrap the blocking calls in
`withContext(Dispatchers.IO)` or use the `...Async` futures with `kotlinx-coroutines-jdk8`.
