using System;
using System.Collections.Generic;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Zanshin.Server
{
    /// <summary>A non-2xx answer from the server (<c>Status</c> is the HTTP status, <c>Code</c> the API error code).</summary>
    public sealed class ZanshinException : Exception
    {
        public int Status { get; }
        public string Code { get; }

        public ZanshinException(int status, string code, string message) : base(message)
        {
            Status = status;
            Code = code;
        }
    }

    public sealed class Reason
    {
        /// <summary>disabled | default | override | experiment | rule | variant | live_event | prerequisite</summary>
        [JsonPropertyName("kind")] public string Kind { get; set; } = "";
        [JsonPropertyName("rule_id")] public string? RuleId { get; set; }
        [JsonPropertyName("variant")] public string? Variant { get; set; }
        [JsonPropertyName("experiment")] public string? Experiment { get; set; }
    }

    public sealed class Flag
    {
        [JsonPropertyName("key")] public string Key { get; set; } = "";
        [JsonPropertyName("value")] public JsonElement Value { get; set; }
        [JsonPropertyName("value_type")] public string ValueType { get; set; } = "";
        [JsonPropertyName("variant")] public string? Variant { get; set; }
        [JsonPropertyName("reason")] public Reason Reason { get; set; } = new Reason();
        [JsonPropertyName("version")] public long Version { get; set; }

        public bool AsBool(bool fallback = false) =>
            Value.ValueKind == JsonValueKind.True ? true : Value.ValueKind == JsonValueKind.False ? false : fallback;

        public string? AsString(string? fallback = null) =>
            Value.ValueKind == JsonValueKind.String ? Value.GetString() : fallback;

        public double AsNumber(double fallback = 0) =>
            Value.ValueKind == JsonValueKind.Number ? Value.GetDouble() : fallback;

        /// <summary>Deserializes the value into <typeparamref name="T"/> (objects, lists, records).</summary>
        public T? As<T>(JsonSerializerOptions? options = null) =>
            Value.ValueKind == JsonValueKind.Undefined ? default : Value.Deserialize<T>(options ?? Json.Options);
    }

    public sealed class Experiment
    {
        [JsonPropertyName("key")] public string Key { get; set; } = "";
        [JsonPropertyName("name")] public string Name { get; set; } = "";
        [JsonPropertyName("variant")] public string Variant { get; set; } = "";
        [JsonPropertyName("value")] public JsonElement Value { get; set; }
        [JsonPropertyName("version")] public long Version { get; set; }
    }

    public sealed class Occurrence
    {
        [JsonPropertyName("start")] public DateTimeOffset Start { get; set; }
        [JsonPropertyName("end")] public DateTimeOffset End { get; set; }
    }

    public sealed class LiveEvent
    {
        [JsonPropertyName("key")] public string Key { get; set; } = "";
        [JsonPropertyName("name")] public string Name { get; set; } = "";
        [JsonPropertyName("description")] public string Description { get; set; } = "";
        [JsonPropertyName("value")] public JsonElement Value { get; set; }
        [JsonPropertyName("active")] public bool Active { get; set; }
        [JsonPropertyName("current")] public Occurrence? Current { get; set; }
        [JsonPropertyName("next")] public Occurrence? Next { get; set; }
        [JsonPropertyName("version")] public long Version { get; set; }
    }

    public sealed class InboxMessage
    {
        [JsonPropertyName("id")] public string Id { get; set; } = "";
        [JsonPropertyName("key")] public string? Key { get; set; }
        [JsonPropertyName("title")] public string Title { get; set; } = "";
        [JsonPropertyName("body")] public string Body { get; set; } = "";
        [JsonPropertyName("image_url")] public string ImageUrl { get; set; } = "";
        [JsonPropertyName("payload")] public JsonElement Payload { get; set; }
        [JsonPropertyName("priority")] public int Priority { get; set; }
        [JsonPropertyName("delivered_at")] public DateTimeOffset DeliveredAt { get; set; }
        [JsonPropertyName("read_at")] public DateTimeOffset? ReadAt { get; set; }
        [JsonPropertyName("expires_at")] public DateTimeOffset? ExpiresAt { get; set; }
    }

    public sealed class DirectMessage
    {
        [JsonPropertyName("title")] public string Title { get; set; } = "";
        [JsonPropertyName("body")] public string? Body { get; set; }
        [JsonPropertyName("image_url")] public string? ImageUrl { get; set; }
        [JsonPropertyName("payload")] public object? Payload { get; set; }
        [JsonPropertyName("priority")] public int? Priority { get; set; }
    }

    public sealed class Event
    {
        public Event() { }

        public Event(string name, string? value = null, IDictionary<string, object?>? metadata = null)
        {
            Name = name;
            Value = value;
            Metadata = metadata;
        }

        [JsonPropertyName("name")] public string Name { get; set; } = "";
        [JsonPropertyName("value")] public string? Value { get; set; }
        [JsonPropertyName("metadata")] public IDictionary<string, object?>? Metadata { get; set; }
        [JsonPropertyName("timestamp")] public DateTimeOffset? Timestamp { get; set; }
        [JsonPropertyName("id")] public string? Id { get; set; }
    }

    public sealed class Properties
    {
        [JsonPropertyName("default")] public Dictionary<string, JsonElement> Default { get; set; } = new Dictionary<string, JsonElement>();
        [JsonPropertyName("computed")] public Dictionary<string, JsonElement> Computed { get; set; } = new Dictionary<string, JsonElement>();
        [JsonPropertyName("custom")] public Dictionary<string, JsonElement> Custom { get; set; } = new Dictionary<string, JsonElement>();
    }

    public sealed class Identity
    {
        [JsonPropertyName("id")] public string Id { get; set; } = "";
        [JsonPropertyName("external_id")] public string ExternalId { get; set; } = "";
        [JsonPropertyName("created_at")] public DateTimeOffset CreatedAt { get; set; }
        [JsonPropertyName("updated_at")] public DateTimeOffset UpdatedAt { get; set; }
        [JsonPropertyName("last_seen_at")] public DateTimeOffset LastSeenAt { get; set; }
        [JsonPropertyName("properties")] public Properties Properties { get; set; } = new Properties();
    }

    /// <summary>A frame from <c>/server/v1/stream</c>.</summary>
    public sealed class Change
    {
        /// <summary>hello | change | resync | pong</summary>
        [JsonPropertyName("type")] public string Type { get; set; } = "";
        [JsonPropertyName("kind")] public string? Kind { get; set; }
        [JsonPropertyName("key")] public string? Key { get; set; }
        [JsonPropertyName("version")] public long Version { get; set; }
        [JsonPropertyName("deleted")] public bool Deleted { get; set; }
        [JsonPropertyName("identity_id")] public string? IdentityId { get; set; }
    }

    internal static class Json
    {
        public static readonly JsonSerializerOptions Options = new JsonSerializerOptions
        {
            PropertyNameCaseInsensitive = true,
            DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
        };
    }
}
