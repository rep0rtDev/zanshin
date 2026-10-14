package dev.zanshin

import java.net.URI
import java.net.URLEncoder
import java.net.http.HttpClient
import java.net.http.WebSocket
import java.nio.charset.StandardCharsets
import java.util.concurrent.CompletionStage
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.min
import kotlin.random.Random

/**
 * Live change stream for one session over the `/v1/stream` WebSocket (JDK 11 `WebSocket`). Keeps a
 * local cache of the latest flags, pings every 30 s, reconnects with exponential backoff and
 * re-authenticates after token refresh.
 *
 * ```kotlin
 * val stream = ZanshinStream(z) { msg -> if (msg is StreamMessage.FlagChanged) apply(msg.flag) }
 * stream.connect()
 * ```
 */
class ZanshinStream(
    private val client: ZanshinClient,
    /** Only receive changes for these flags (experiments / live events / inbox are always pushed). */
    private val names: List<String>? = null,
    private val reconnect: Boolean = true,
    private val http: HttpClient = HttpClient.newHttpClient(),
    private val listener: (StreamMessage) -> Unit,
) {
    private val scheduler = Executors.newSingleThreadScheduledExecutor { r -> Thread(r, "zanshin-stream").apply { isDaemon = true } }
    @Volatile private var ws: WebSocket? = null
    @Volatile private var closed = true
    private val attempt = AtomicInteger()
    private var pingTask: ScheduledFuture<*>? = null
    private val flagMap = java.util.concurrent.ConcurrentHashMap<String, Flag>()

    /** Latest known flags by key (updated from the stream). */
    val flags: Map<String, Flag> get() = flagMap
    val connected: Boolean get() = ws?.let { !it.isInputClosed } ?: false

    fun connect() {
        closed = false
        open()
    }

    /** Stop and do not reconnect. */
    fun close() {
        closed = true
        pingTask?.cancel(false)
        ws?.sendClose(WebSocket.NORMAL_CLOSURE, "bye")
        ws = null
        scheduler.shutdownNow()
    }

    private fun wsUrl(token: String): URI {
        var base = client.url
        base = if (base.startsWith("https")) "wss" + base.substring(5) else "ws" + base.substring(4)
        val q = StringBuilder("token=").append(URLEncoder.encode(token, StandardCharsets.UTF_8))
        if (!names.isNullOrEmpty()) q.append("&names=").append(URLEncoder.encode(names.joinToString(","), StandardCharsets.UTF_8))
        return URI.create("$base/v1/stream?$q")
    }

    private fun open() {
        if (closed) return
        val token = client.token ?: run { scheduleReconnect(); return }
        val buf = StringBuilder()
        val l = object : WebSocket.Listener {
            override fun onOpen(webSocket: WebSocket) {
                attempt.set(0)
                pingTask = scheduler.scheduleAtFixedRate({ ws?.sendText("{\"type\":\"ping\"}", true) }, 30, 30, TimeUnit.SECONDS)
                webSocket.request(1)
            }

            override fun onText(webSocket: WebSocket, data: CharSequence, last: Boolean): CompletionStage<*>? {
                buf.append(data)
                if (last) {
                    val text = buf.toString()
                    buf.setLength(0)
                    dispatch(text)
                }
                webSocket.request(1)
                return null
            }

            override fun onClose(webSocket: WebSocket, statusCode: Int, reason: String): CompletionStage<*>? {
                handleClose(); return null
            }

            override fun onError(webSocket: WebSocket, error: Throwable) { handleClose() }
        }
        http.newWebSocketBuilder().buildAsync(wsUrl(token), l).whenComplete { socket, err ->
            if (err != null) handleClose() else ws = socket
        }
    }

    private fun handleClose() {
        pingTask?.cancel(false)
        ws = null
        scheduleReconnect()
    }

    private fun scheduleReconnect() {
        if (closed || !reconnect || scheduler.isShutdown) return
        val n = attempt.getAndIncrement()
        val delayMs = (min(30_000.0, 1000.0 * Math.pow(2.0, min(n, 5).toDouble())) * (0.5 + Random.nextDouble())).toLong()
        scheduler.schedule({
            if (closed) return@schedule
            if (client.token == null) {
                try { client.refresh() } catch (_: Exception) { scheduleReconnect(); return@schedule }
            }
            open()
        }, delayMs, TimeUnit.MILLISECONDS)
    }

    private fun dispatch(text: String) {
        val raw = try { Json.parse(text).asMap() } catch (_: Exception) { return }
        val msg: StreamMessage = when (val type = raw["type"].asString() ?: return) {
            "hello" -> StreamMessage.Hello(raw["identity_id"].asString() ?: "")
            "flag" -> Flag.from(raw["flag"].asMap()).let { flagMap[it.key] = it; StreamMessage.FlagChanged(it) }
            "flag_deleted" -> (raw["key"].asString() ?: "").let { flagMap.remove(it); StreamMessage.FlagDeleted(it) }
            "flags" -> raw["flags"].asList().map { Flag.from(it.asMap()) }.let { fs -> fs.forEach { flagMap[it.key] = it }; StreamMessage.Flags(fs) }
            "experiments" -> StreamMessage.Experiments(raw["experiments"].asList().map { Experiment.from(it.asMap()) })
            "live_events" -> StreamMessage.LiveEvents(raw["live_events"].asList().map { LiveEvent.from(it.asMap()) })
            "inbox" -> StreamMessage.Inbox(raw["messages"].asList().map { InboxMessage.from(it.asMap()) })
            "pong" -> StreamMessage.Pong
            "error" -> {
                val m = raw["message"].asString() ?: ""
                if (m.contains("expired", ignoreCase = true)) scheduler.execute { try { client.refresh() } catch (_: Exception) {} }
                StreamMessage.Error(m)
            }
            else -> StreamMessage.Unknown(type)
        }
        try { listener(msg) } catch (_: Exception) { /* listener errors must not kill the stream */ }
    }
}
