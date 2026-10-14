package dev.zanshin

/** Map of property name to JSON value (null, Boolean, Double, String, List, Map). */
typealias PropertyMap = Map<String, Any?>

data class Properties(val default: PropertyMap, val computed: PropertyMap, val custom: PropertyMap) {
    companion object {
        fun from(m: Map<String, Any?>) = Properties(m["default"].asMap(), m["computed"].asMap(), m["custom"].asMap())
    }
}

data class Identity(val id: String, val externalId: String, val createdAt: String, val updatedAt: String, val lastSeenAt: String) {
    companion object {
        fun from(m: Map<String, Any?>) = Identity(
            m["id"].asString() ?: "", m["external_id"].asString() ?: "", m["created_at"].asString() ?: "",
            m["updated_at"].asString() ?: "", m["last_seen_at"].asString() ?: "",
        )
    }
}

data class Session(val token: String, val refreshToken: String, val identity: Identity, val properties: Properties) {
    companion object {
        fun from(m: Map<String, Any?>) = Session(
            m["token"].asString() ?: "", m["refresh_token"].asString() ?: "",
            Identity.from(m["identity"].asMap()), Properties.from(m["properties"].asMap()),
        )
    }
}

/** kind: disabled | default | override | experiment | rule | variant */
data class Reason(val kind: String, val ruleId: String?, val variant: String?, val experiment: String?) {
    companion object {
        fun from(m: Map<String, Any?>) = Reason(m["kind"].asString() ?: "default", m["rule_id"].asString(), m["variant"].asString(), m["experiment"].asString())
    }
}

data class Flag(val key: String, val value: Any?, val valueType: String, val variant: String?, val reason: Reason, val version: Long) {
    val boolValue: Boolean? get() = value as? Boolean
    val stringValue: String? get() = value as? String
    val numberValue: Double? get() = (value as? Number)?.toDouble()
    val objectValue: Map<String, Any?>? get() = @Suppress("UNCHECKED_CAST") (value as? Map<String, Any?>)

    companion object {
        fun from(m: Map<String, Any?>) = Flag(
            m["key"].asString() ?: "", m["value"], m["value_type"].asString() ?: "json", m["variant"].asString(),
            Reason.from(m["reason"].asMap()), m["version"].asLong(),
        )
    }
}

/** source: identity | experiment | live_event */
data class FlagOverride(val flagKey: String, val value: Any?, val source: String, val sourceKey: String?, val variant: String?) {
    companion object {
        fun from(m: Map<String, Any?>) = FlagOverride(m["flag_key"].asString() ?: "", m["value"], m["source"].asString() ?: "", m["source_key"].asString(), m["variant"].asString())
    }
}

data class Experiment(val key: String, val name: String, val variant: String, val value: Any?, val version: Long) {
    companion object {
        fun from(m: Map<String, Any?>) = Experiment(m["key"].asString() ?: "", m["name"].asString() ?: "", m["variant"].asString() ?: "", m["value"], m["version"].asLong())
    }
}

data class Occurrence(val start: String, val end: String) {
    companion object {
        fun from(m: Map<String, Any?>?) = m?.let { Occurrence(it["start"].asString() ?: "", it["end"].asString() ?: "") }
    }
}

data class LiveEvent(val key: String, val name: String, val description: String, val value: Any?, val active: Boolean, val current: Occurrence?, val next: Occurrence?, val version: Long) {
    companion object {
        @Suppress("UNCHECKED_CAST")
        fun from(m: Map<String, Any?>) = LiveEvent(
            m["key"].asString() ?: "", m["name"].asString() ?: "", m["description"].asString() ?: "", m["value"], m["active"].asBool(),
            Occurrence.from(m["current"] as? Map<String, Any?>), Occurrence.from(m["next"] as? Map<String, Any?>), m["version"].asLong(),
        )
    }
}

data class InboxMessage(
    val id: String, val key: String?, val title: String, val body: String, val imageUrl: String, val payload: Any?,
    val priority: Int, val deliveredAt: String, val readAt: String?, val expiresAt: String?,
) {
    companion object {
        fun from(m: Map<String, Any?>) = InboxMessage(
            m["id"].asString() ?: "", m["key"].asString(), m["title"].asString() ?: "", m["body"].asString() ?: "",
            m["image_url"].asString() ?: "", m["payload"], m["priority"].asInt(), m["delivered_at"].asString() ?: "",
            m["read_at"].asString(), m["expires_at"].asString(),
        )
    }
}

/** platform: fcm | apns */
data class PushToken(val identityId: String, val platform: String, val token: String, val deviceId: String, val createdAt: String, val updatedAt: String, val failures: Int) {
    companion object {
        fun from(m: Map<String, Any?>) = PushToken(
            m["identity_id"].asString() ?: "", m["platform"].asString() ?: "", m["token"].asString() ?: "", m["device_id"].asString() ?: "",
            m["created_at"].asString() ?: "", m["updated_at"].asString() ?: "", m["failures"].asInt(),
        )
    }
}

/** Analytics event; [timestamp] is ISO-8601 (null = server receipt time), [id] an idempotency UUID. */
data class EventInput(
    val name: String,
    val value: String? = null,
    val metadata: Map<String, Any?>? = null,
    val timestamp: String? = null,
    val id: String? = null,
) : JsonSerializable {
    override fun toJson(): Any? = buildMap<String, Any?> {
        put("name", name)
        value?.let { put("value", it) }
        metadata?.let { put("metadata", it) }
        timestamp?.let { put("timestamp", it) }
        id?.let { put("id", it) }
    }
}

/** Error returned by the server or raised by the client. */
class ZanshinException(val status: Int, val code: String, message: String) : RuntimeException(message) {
    val isNotFound: Boolean get() = status == 404
    val isUnauthenticated: Boolean get() = status == 401
    override fun toString() = "ZanshinException($code/$status: $message)"
}

/** Messages pushed on the `/v1/stream` WebSocket. */
sealed class StreamMessage {
    data class Hello(val identityId: String) : StreamMessage()
    data class FlagChanged(val flag: Flag) : StreamMessage()
    data class FlagDeleted(val key: String) : StreamMessage()
    data class Flags(val flags: List<Flag>) : StreamMessage()
    data class Experiments(val experiments: List<Experiment>) : StreamMessage()
    data class LiveEvents(val liveEvents: List<LiveEvent>) : StreamMessage()
    data class Inbox(val messages: List<InboxMessage>) : StreamMessage()
    object Pong : StreamMessage()
    data class Error(val message: String) : StreamMessage()
    data class Unknown(val type: String) : StreamMessage()
}
