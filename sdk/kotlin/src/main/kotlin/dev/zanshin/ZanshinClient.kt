package dev.zanshin

import java.net.URI
import java.net.URLEncoder
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.nio.charset.StandardCharsets
import java.time.Duration
import java.util.Base64
import java.util.concurrent.CompletableFuture

/**
 * Zanshin client for a single identity (one player / device). Blocking calls on top of the JDK 11
 * `HttpClient`; every method has a `...Async` twin returning a [CompletableFuture]. Thread-safe.
 *
 * ```kotlin
 * val z = ZanshinClient("https://liveops.example.com", "zk_c_...")
 * z.authenticate(deviceId, default = mapOf("platform" to "android"))
 * val shop = z.flag("shop_v2").boolValue ?: false
 * ```
 */
class ZanshinClient(
    url: String,
    private val apiKey: String,
    private val http: HttpClient = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(10)).build(),
    /** Refresh the access token this many seconds before it expires. */
    private val refreshLeewaySeconds: Long = 60,
    /** Called whenever tokens change (persist them to resume the session later); null on logout. */
    private val onSession: ((Tokens?) -> Unit)? = null,
) {
    data class Tokens(val token: String, val refreshToken: String)

    val url: String = url.trimEnd('/')

    @Volatile private var tokens: Tokens? = null
    @Volatile private var expiresAt: Long = 0
    private val refreshLock = Any()
    private var refreshing: CompletableFuture<Session>? = null

    /** Current access token, or null when not authenticated. */
    val token: String? get() = tokens?.token
    val isAuthenticated: Boolean get() = tokens != null

    // ---- session ------------------------------------------------------------------------------

    /** Resume a persisted session; refreshes immediately if the access token is stale. */
    fun restore(token: String, refreshToken: String) {
        setTokens(Tokens(token, refreshToken), notify = false)
        if (expiringSoon()) refresh()
    }

    /** Authenticate (create or resume) the identity [id] and open a session. */
    fun authenticate(id: String, default: PropertyMap = emptyMap(), custom: PropertyMap = emptyMap()): Session =
        Session.from(request("POST", "/v1/authenticate", Auth.KEY, body = mapOf("id" to id, "default" to default, "custom" to custom)).asMap())
            .also { setTokens(Tokens(it.token, it.refreshToken)) }

    /** Rotate tokens. Concurrent calls share one request. */
    fun refresh(): Session = refreshAsync().join()

    fun refreshAsync(): CompletableFuture<Session> {
        synchronized(refreshLock) {
            refreshing?.let { return it }
            val rt = tokens?.refreshToken
                ?: return CompletableFuture.failedFuture(ZanshinException(401, "unauthenticated", "no refresh token"))
            val f = CompletableFuture.supplyAsync {
                try {
                    Session.from(request("POST", "/v1/authenticate/refresh", Auth.KEY, body = mapOf("refresh_token" to rt)).asMap())
                        .also { setTokens(Tokens(it.token, it.refreshToken)) }
                } catch (e: ZanshinException) {
                    if (e.status == 401) clearTokens()
                    throw e
                }
            }
            f.whenComplete { _, _ -> synchronized(refreshLock) { refreshing = null } }
            refreshing = f
            return f
        }
    }

    /** Revoke the session server-side and forget tokens. */
    fun logout() {
        val t = tokens ?: return
        clearTokens()
        request("POST", "/v1/authenticate/logout", Auth.KEY, body = mapOf("token" to t.token, "refresh_token" to t.refreshToken))
    }

    // ---- identity & properties ----------------------------------------------------------------

    fun identity(): Identity = Identity.from(request("GET", "/v1/identity").asMap())

    /** Delete the identity and everything attached to it. */
    fun deleteIdentity() { request("DELETE", "/v1/identity"); clearTokens() }

    fun properties(): Properties = Properties.from(request("GET", "/v1/properties").asMap())

    /** Merge properties; a null value removes the key. */
    fun updateProperties(default: PropertyMap = emptyMap(), custom: PropertyMap = emptyMap()): Properties =
        Properties.from(request("PUT", "/v1/properties", body = mapOf("default" to default, "custom" to custom)).asMap())

    /** Link the current (typically anonymous) identity to [id]; the client switches to the new session. */
    fun identify(id: String): Session =
        Session.from(request("POST", "/v1/identify", body = mapOf("id" to id)).asMap()).also { setTokens(Tokens(it.token, it.refreshToken)) }

    // ---- flags --------------------------------------------------------------------------------

    fun flags(names: List<String>? = null): List<Flag> =
        request("GET", "/v1/flags", query = mapOf("names" to names?.joinToString(","))).asMap()["flags"].asList().map { Flag.from(it.asMap()) }

    fun flag(key: String): Flag = Flag.from(request("GET", "/v1/flags/" + enc(key)).asMap())

    /** Flag value with a fallback when the flag does not exist. */
    fun <T> flagValue(key: String, fallback: T): T = try {
        @Suppress("UNCHECKED_CAST") (flag(key).value as? T) ?: fallback
    } catch (e: ZanshinException) {
        if (e.isNotFound) fallback else throw e
    }

    /** Every identity / experiment / live-event override applied to the caller right now. */
    fun flagOverrides(): List<FlagOverride> =
        request("GET", "/v1/flags/overrides").asMap()["overrides"].asList().map { FlagOverride.from(it.asMap()) }

    fun audiences(): List<String> = request("GET", "/v1/audiences").asMap()["audiences"].asList().mapNotNull { it.asString() }

    // ---- experiments --------------------------------------------------------------------------

    fun experiments(names: List<String>? = null): List<Experiment> =
        request("GET", "/v1/experiments", query = mapOf("names" to names?.joinToString(","))).asMap()["experiments"].asList().map { Experiment.from(it.asMap()) }

    /** Record that the identity actually saw its variant. */
    fun expose(experiment: String) { request("POST", "/v1/experiments/" + enc(experiment) + "/expose") }

    // ---- live events --------------------------------------------------------------------------

    fun liveEvents(names: List<String>? = null, upcoming: Boolean = false): List<LiveEvent> =
        request("GET", "/v1/live-events", query = mapOf("names" to names?.joinToString(","), "upcoming" to if (upcoming) "true" else null))
            .asMap()["live_events"].asList().map { LiveEvent.from(it.asMap()) }

    /** Explicitly join a live event with `join_mode: explicit`; returns the event as now seen. */
    fun joinLiveEvent(key: String): LiveEvent = LiveEvent.from(request("POST", "/v1/live-events/" + enc(key) + "/join").asMap())

    /** Leave a live event; false when the identity was not a participant. */
    fun leaveLiveEvent(key: String): Boolean = request("POST", "/v1/live-events/" + enc(key) + "/leave").asMap()["left"].asBool()

    // ---- inbox --------------------------------------------------------------------------------

    fun inbox(limit: Int? = null): List<InboxMessage> =
        request("GET", "/v1/messages", query = mapOf("limit" to limit?.toString())).asMap()["messages"].asList().map { InboxMessage.from(it.asMap()) }

    fun markRead(id: String): InboxMessage = InboxMessage.from(request("PUT", "/v1/messages/" + enc(id), body = mapOf("read" to true)).asMap())

    /** Consume (remove) a message; it is never redelivered. */
    fun consume(id: String) { request("DELETE", "/v1/messages/" + enc(id)) }

    /** Remove a message from the inbox entirely. */
    fun deleteInboxMessage(id: String) { request("DELETE", "/v1/inbox/" + enc(id)) }

    // ---- push tokens --------------------------------------------------------------------------

    /** Register this device for push notifications (`fcm` registration token or `apns` hex token). */
    fun registerPushToken(platform: String, token: String, deviceId: String = ""): PushToken =
        PushToken.from(request("PUT", "/v1/push-tokens", body = mapOf("platform" to platform, "token" to token, "device_id" to deviceId)).asMap())

    fun pushTokens(): List<PushToken> = request("GET", "/v1/push-tokens").asMap()["tokens"].asList().map { PushToken.from(it.asMap()) }

    fun unregisterPushToken(token: String) { request("DELETE", "/v1/push-tokens/" + enc(token)) }

    // ---- analytics ----------------------------------------------------------------------------

    /** Report events; returns how many were accepted. */
    fun track(vararg events: EventInput): Int {
        if (events.isEmpty()) return 0
        return request("POST", "/v1/events", body = mapOf("events" to events.toList())).asMap()["accepted"].asInt()
    }

    /** Convenience: one event. */
    fun event(name: String, value: Any? = null, metadata: Map<String, Any?>? = null): Int =
        track(EventInput(name, value?.toString(), metadata))

    // ---- async twins --------------------------------------------------------------------------

    fun authenticateAsync(id: String, default: PropertyMap = emptyMap(), custom: PropertyMap = emptyMap()): CompletableFuture<Session> =
        CompletableFuture.supplyAsync { authenticate(id, default, custom) }
    fun flagsAsync(names: List<String>? = null): CompletableFuture<List<Flag>> = CompletableFuture.supplyAsync { flags(names) }
    fun flagAsync(key: String): CompletableFuture<Flag> = CompletableFuture.supplyAsync { flag(key) }
    fun experimentsAsync(names: List<String>? = null): CompletableFuture<List<Experiment>> = CompletableFuture.supplyAsync { experiments(names) }
    fun liveEventsAsync(names: List<String>? = null, upcoming: Boolean = false): CompletableFuture<List<LiveEvent>> =
        CompletableFuture.supplyAsync { liveEvents(names, upcoming) }
    fun inboxAsync(limit: Int? = null): CompletableFuture<List<InboxMessage>> = CompletableFuture.supplyAsync { inbox(limit) }
    fun trackAsync(vararg events: EventInput): CompletableFuture<Int> = CompletableFuture.supplyAsync { track(*events) }

    // ---- transport ----------------------------------------------------------------------------

    internal enum class Auth { KEY, SESSION, NONE }

    private fun setTokens(t: Tokens, notify: Boolean = true) {
        tokens = t
        expiresAt = decodeExp(t.token)
        if (notify) onSession?.invoke(t)
    }

    private fun clearTokens() {
        tokens = null
        expiresAt = 0
        onSession?.invoke(null)
    }

    private fun expiringSoon(): Boolean {
        if (tokens == null) return true
        if (expiresAt == 0L) return false
        return System.currentTimeMillis() / 1000 >= expiresAt - refreshLeewaySeconds
    }

    private fun enc(s: String) = URLEncoder.encode(s, StandardCharsets.UTF_8).replace("+", "%20")

    internal fun request(method: String, path: String, auth: Auth = Auth.SESSION, query: Map<String, String?> = emptyMap(), body: Any? = null): Any? {
        val q = query.filterValues { !it.isNullOrEmpty() }.entries.joinToString("&") { enc(it.key) + "=" + enc(it.value!!) }
        val b = HttpRequest.newBuilder(URI.create(url + path + if (q.isEmpty()) "" else "?$q"))
            .timeout(Duration.ofSeconds(30))
            .header("accept", "application/json")
        when (auth) {
            Auth.KEY -> b.header("x-api-key", apiKey)
            Auth.SESSION -> {
                if (tokens == null) throw ZanshinException(401, "unauthenticated", "not authenticated")
                if (expiringSoon() && !tokens?.refreshToken.isNullOrEmpty()) refresh()
                b.header("authorization", "Bearer " + (tokens?.token ?: ""))
            }
            Auth.NONE -> {}
        }
        if (body != null) {
            b.header("content-type", "application/json")
            b.method(method, HttpRequest.BodyPublishers.ofString(Json.stringify(body)))
        } else {
            b.method(method, HttpRequest.BodyPublishers.noBody())
        }
        val res = http.send(b.build(), HttpResponse.BodyHandlers.ofString())
        val text = res.body() ?: ""
        if (res.statusCode() !in 200..299) {
            var code = "error"
            var message = text
            try {
                val e = Json.parse(text).asMap()
                e["code"].asString()?.let { code = it }
                e["message"].asString()?.let { message = it }
            } catch (_: Exception) { /* non-JSON error body */ }
            if (res.statusCode() == 401 && auth == Auth.SESSION) clearTokens()
            throw ZanshinException(res.statusCode(), code, message)
        }
        return if (text.isEmpty()) emptyMap<String, Any?>() else Json.parse(text)
    }

    companion object {
        /** `exp` claim of a JWT (unix seconds), 0 when absent. */
        fun decodeExp(jwt: String): Long = try {
            val payload = jwt.split('.').getOrNull(1) ?: return 0
            val json = String(Base64.getUrlDecoder().decode(payload.trimEnd('=')), StandardCharsets.UTF_8)
            Json.parse(json).asMap()["exp"].asLong()
        } catch (_: Exception) { 0 }
    }
}
