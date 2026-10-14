using System;
using System.Collections.Generic;

namespace Zanshin
{
    public class ZanshinException : Exception
    {
        public int Status { get; }
        public string Code { get; }
        public bool IsNotFound => Status == 404;
        public bool IsUnauthenticated => Status == 401;

        public ZanshinException(int status, string code, string message) : base(message)
        {
            Status = status;
            Code = code;
        }
    }

    public sealed class Identity
    {
        public string Id, ExternalId, CreatedAt, UpdatedAt, LastSeenAt;

        internal static Identity From(object o) => o == null ? null : new Identity
        {
            Id = Json.Str(o, "id"), ExternalId = Json.Str(o, "external_id"), CreatedAt = Json.Str(o, "created_at"),
            UpdatedAt = Json.Str(o, "updated_at"), LastSeenAt = Json.Str(o, "last_seen_at")
        };
    }

    public sealed class Properties
    {
        public Dictionary<string, object> Default = new Dictionary<string, object>();
        public Dictionary<string, object> Computed = new Dictionary<string, object>();
        public Dictionary<string, object> Custom = new Dictionary<string, object>();

        internal static Properties From(object o) => new Properties
        {
            Default = Json.Obj(Json.Get(o, "default")) ?? new Dictionary<string, object>(),
            Computed = Json.Obj(Json.Get(o, "computed")) ?? new Dictionary<string, object>(),
            Custom = Json.Obj(Json.Get(o, "custom")) ?? new Dictionary<string, object>()
        };
    }

    public sealed class Session
    {
        public string Token, RefreshToken;
        public Identity Identity;
        public Properties Properties;

        internal static Session From(object o) => new Session
        {
            Token = Json.Str(o, "token"), RefreshToken = Json.Str(o, "refresh_token"),
            Identity = Identity.From(Json.Get(o, "identity")), Properties = Properties.From(Json.Get(o, "properties"))
        };
    }

    public sealed class Flag
    {
        public string Key, ValueType, Variant, ReasonKind, Experiment;
        public object Value;
        public long Version;

        public bool AsBool(bool dflt = false) => Value is bool b ? b : dflt;
        public string AsString(string dflt = null) => Value is string s ? s : Value == null ? dflt : Json.Serialize(Value);
        public double AsNumber(double dflt = 0) => Value is double d ? d : Value is long l ? l : dflt;
        public Dictionary<string, object> AsObject() => Json.Obj(Value);

        internal static Flag From(object o)
        {
            var reason = Json.Get(o, "reason");
            return new Flag
            {
                Key = Json.Str(o, "key"), ValueType = Json.Str(o, "value_type"), Value = Json.Get(o, "value"),
                Variant = Json.Str(o, "variant"), Version = Json.Int(o, "version"),
                ReasonKind = Json.Str(reason, "kind"), Experiment = Json.Str(reason, "experiment")
            };
        }
    }

    public sealed class Experiment
    {
        public string Key, Name, Variant;
        public object Value;
        public long Version;

        internal static Experiment From(object o) => new Experiment
        {
            Key = Json.Str(o, "key"), Name = Json.Str(o, "name"), Variant = Json.Str(o, "variant"),
            Value = Json.Get(o, "value"), Version = Json.Int(o, "version")
        };
    }

    public sealed class Occurrence
    {
        public DateTime Start, End;

        internal static Occurrence From(object o) => o == null ? null : new Occurrence
        {
            Start = ParseTime(Json.Str(o, "start")), End = ParseTime(Json.Str(o, "end"))
        };

        internal static DateTime ParseTime(string s) =>
            string.IsNullOrEmpty(s) ? default : DateTime.Parse(s, null, System.Globalization.DateTimeStyles.AdjustToUniversal | System.Globalization.DateTimeStyles.AssumeUniversal);
    }

    public sealed class LiveEvent
    {
        public string Key, Name, Description;
        public object Value;
        public bool Active;
        public Occurrence Current, Next;
        public long Version;

        internal static LiveEvent From(object o) => new LiveEvent
        {
            Key = Json.Str(o, "key"), Name = Json.Str(o, "name"), Description = Json.Str(o, "description"),
            Value = Json.Get(o, "value"), Active = Json.Bool(o, "active"),
            Current = Occurrence.From(Json.Get(o, "current")), Next = Occurrence.From(Json.Get(o, "next")), Version = Json.Int(o, "version")
        };
    }

    public sealed class InboxMessage
    {
        public string Id, Key, Title, Body, ImageUrl, DeliveredAt, ReadAt, ExpiresAt;
        public object Payload;
        public long Priority;
        public bool IsRead => ReadAt != null;

        internal static InboxMessage From(object o) => new InboxMessage
        {
            Id = Json.Str(o, "id"), Key = Json.Str(o, "key"), Title = Json.Str(o, "title"), Body = Json.Str(o, "body"),
            ImageUrl = Json.Str(o, "image_url"), Payload = Json.Get(o, "payload"), Priority = Json.Int(o, "priority"),
            DeliveredAt = Json.Str(o, "delivered_at"), ReadAt = Json.Str(o, "read_at"), ExpiresAt = Json.Str(o, "expires_at")
        };
    }

    public sealed class PushToken
    {
        public string Platform, Token, DeviceId;
        public long Failures;

        internal static PushToken From(object o) => new PushToken
        {
            Platform = Json.Str(o, "platform"), Token = Json.Str(o, "token"), DeviceId = Json.Str(o, "device_id"), Failures = Json.Int(o, "failures")
        };
    }

    public sealed class Event
    {
        public string Name;
        public string Value;
        public Dictionary<string, object> Metadata;
        /// <summary>Optional; defaults to server receipt time.</summary>
        public DateTime? Timestamp;
        /// <summary>Optional idempotency UUID.</summary>
        public string Id;

        public Event(string name, string value = null, Dictionary<string, object> metadata = null)
        {
            Name = name;
            Value = value;
            Metadata = metadata;
        }

        internal Dictionary<string, object> ToJson()
        {
            var d = new Dictionary<string, object> { ["name"] = Name };
            if (Value != null) d["value"] = Value;
            if (Metadata != null) d["metadata"] = Metadata;
            if (Timestamp.HasValue) d["timestamp"] = Timestamp.Value.ToUniversalTime().ToString("o");
            if (Id != null) d["id"] = Id;
            return d;
        }
    }

    internal static class Lists
    {
        public static List<T> Map<T>(object arr, Func<object, T> f)
        {
            var r = new List<T>();
            var l = Json.Arr(arr);
            if (l != null) foreach (var x in l) r.Add(f(x));
            return r;
        }
    }
}
