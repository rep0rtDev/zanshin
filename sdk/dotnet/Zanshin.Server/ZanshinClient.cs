using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Linq;
using System.Net.Http;
using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;

namespace Zanshin.Server
{
    public sealed class ZanshinOptions
    {
        /// <summary>Bring your own <see cref="HttpClient"/> (pooling, proxies, custom handlers). Default: a 10 s timeout client.</summary>
        public HttpClient? Http { get; set; }

        /// <summary>How long a per-identity answer (flags, audiences, …) is served from memory. The change stream invalidates earlier.</summary>
        public TimeSpan CacheTtl { get; set; } = TimeSpan.FromSeconds(30);

        /// <summary>Optional diagnostics sink.</summary>
        public Action<string>? Log { get; set; }
    }

    /// <summary>
    /// Server-key client for the Zanshin server API (<c>/server/v1</c>): evaluates flags, experiments,
    /// audiences, live events and inbox for any player by external id, tracks events and sends messages.
    /// Per-identity reads are cached for <see cref="ZanshinOptions.CacheTtl"/>; call <see cref="StartStream"/>
    /// to drop the cache the moment something changes in the console.
    /// </summary>
    public sealed class ZanshinClient : IDisposable
    {
        private readonly string _baseUrl;
        private readonly string _apiKey;
        private readonly HttpClient _http;
        private readonly TimeSpan _ttl;
        private readonly Action<string> _log;
        private readonly ConcurrentDictionary<string, CacheEntry> _cache = new ConcurrentDictionary<string, CacheEntry>();
        private long _gen;
        private CancellationTokenSource? _stream;

        private sealed class CacheEntry
        {
            public DateTime At;
            public long Gen;
            public string Body = "";
        }

        public ZanshinClient(string baseUrl, string serverKey, ZanshinOptions? options = null)
        {
            if (string.IsNullOrEmpty(baseUrl)) throw new ArgumentException("baseUrl is required", nameof(baseUrl));
            if (string.IsNullOrEmpty(serverKey)) throw new ArgumentException("serverKey is required", nameof(serverKey));
            options ??= new ZanshinOptions();
            _baseUrl = baseUrl.TrimEnd('/');
            _apiKey = serverKey;
            _http = options.Http ?? new HttpClient { Timeout = TimeSpan.FromSeconds(10) };
            _ttl = options.CacheTtl;
            _log = options.Log ?? (_ => { });
        }

        /// <summary>Raised for every <c>change</c> / <c>resync</c> frame once <see cref="StartStream"/> is running.</summary>
        public event Action<Change>? OnChange;

        /// <summary>Drops every cached answer.</summary>
        public void Invalidate()
        {
            Interlocked.Increment(ref _gen);
            _cache.Clear();
        }

        /// <summary>Drops cached answers for one player (e.g. after you changed their properties elsewhere).</summary>
        public void InvalidateIdentity(string externalId)
        {
            foreach (var k in _cache.Keys)
            {
                var parts = k.Split(new[] { '\0' }, 3);
                if (parts.Length >= 2 && parts[1] == externalId) _cache.TryRemove(k, out _);
            }
        }

        // ---- identities ------------------------------------------------------------------------

        public async Task<Identity> UpsertIdentityAsync(string externalId, IDictionary<string, object?>? defaultProps = null,
            IDictionary<string, object?>? customProps = null, CancellationToken ct = default)
        {
            var body = new Dictionary<string, object?> { ["external_id"] = externalId };
            if (defaultProps != null) body["default"] = defaultProps;
            if (customProps != null) body["custom"] = customProps;
            var doc = await RequestAsync(HttpMethod.Post, "/server/v1/identities", null, body, ct).ConfigureAwait(false);
            InvalidateIdentity(externalId);
            return Parse<Identity>(doc);
        }

        public async Task<Properties> UpdatePropertiesAsync(string externalId, IDictionary<string, object?>? defaultProps = null,
            IDictionary<string, object?>? customProps = null, CancellationToken ct = default)
        {
            var body = new Dictionary<string, object?>();
            if (defaultProps != null) body["default"] = defaultProps;
            if (customProps != null) body["custom"] = customProps;
            var doc = await RequestAsync(HttpMethod.Put, "/server/v1/identities/" + Uri.EscapeDataString(externalId) + "/properties", null, body, ct).ConfigureAwait(false);
            InvalidateIdentity(externalId);
            return Parse<Properties>(doc);
        }

        // ---- reads (cached) ---------------------------------------------------------------------

        public async Task<IReadOnlyList<Flag>> FlagsAsync(string externalId, IEnumerable<string>? names = null, CancellationToken ct = default)
        {
            var q = IdQuery(externalId, names);
            var body = await CachedAsync("flags", externalId, q, ct).ConfigureAwait(false);
            return ParseList<Flag>(body, "flags");
        }

        public async Task<Flag> FlagAsync(string externalId, string name, CancellationToken ct = default)
        {
            var q = IdQuery(externalId, null);
            var body = await CachedAsync("flag:" + name, externalId, q, ct, "/server/v1/flags/" + Uri.EscapeDataString(name)).ConfigureAwait(false);
            return Parse<Flag>(body);
        }

        public async Task<bool> FlagBoolAsync(string externalId, string name, bool fallback = false, CancellationToken ct = default) =>
            (await FlagOrNullAsync(externalId, name, ct).ConfigureAwait(false))?.AsBool(fallback) ?? fallback;

        public async Task<string?> FlagStringAsync(string externalId, string name, string? fallback = null, CancellationToken ct = default) =>
            (await FlagOrNullAsync(externalId, name, ct).ConfigureAwait(false))?.AsString(fallback) ?? fallback;

        public async Task<double> FlagNumberAsync(string externalId, string name, double fallback = 0, CancellationToken ct = default) =>
            (await FlagOrNullAsync(externalId, name, ct).ConfigureAwait(false))?.AsNumber(fallback) ?? fallback;

        private async Task<Flag?> FlagOrNullAsync(string externalId, string name, CancellationToken ct)
        {
            try { return await FlagAsync(externalId, name, ct).ConfigureAwait(false); }
            catch (ZanshinException e) when (e.Status == 404) { return null; }
        }

        public async Task<IReadOnlyList<string>> AudiencesAsync(string externalId, CancellationToken ct = default)
        {
            var body = await CachedAsync("audiences", externalId, IdQuery(externalId, null), ct).ConfigureAwait(false);
            return ParseList<string>(body, "audiences");
        }

        public async Task<IReadOnlyList<Experiment>> ExperimentsAsync(string externalId, IEnumerable<string>? names = null, CancellationToken ct = default)
        {
            var body = await CachedAsync("experiments", externalId, IdQuery(externalId, names), ct).ConfigureAwait(false);
            return ParseList<Experiment>(body, "experiments");
        }

        public async Task<IReadOnlyList<LiveEvent>> LiveEventsAsync(string externalId, bool upcoming = false, IEnumerable<string>? names = null, CancellationToken ct = default)
        {
            var q = IdQuery(externalId, names);
            if (upcoming) q["upcoming"] = "true";
            var body = await CachedAsync("live-events", externalId, q, ct).ConfigureAwait(false);
            return ParseList<LiveEvent>(body, "live_events");
        }

        public async Task<IReadOnlyList<InboxMessage>> InboxAsync(string externalId, int? limit = null, CancellationToken ct = default)
        {
            var q = IdQuery(externalId, null);
            if (limit.HasValue) q["limit"] = limit.Value.ToString();
            var body = await CachedAsync("messages", externalId, q, ct).ConfigureAwait(false);
            return ParseList<InboxMessage>(body, "messages");
        }

        // ---- writes -----------------------------------------------------------------------------

        public async Task ExposeAsync(string externalId, string experiment, CancellationToken ct = default)
        {
            await RequestAsync(HttpMethod.Post, "/server/v1/experiments/" + Uri.EscapeDataString(experiment) + "/expose", IdQuery(externalId, null), null, ct).ConfigureAwait(false);
        }

        /// <summary>Sends an inbox message to one player; returns the message id.</summary>
        public async Task<string> SendMessageAsync(string externalId, DirectMessage message, CancellationToken ct = default)
        {
            var body = new Dictionary<string, object?>
            {
                ["external_id"] = externalId,
                ["title"] = message.Title,
                ["body"] = message.Body,
                ["image_url"] = message.ImageUrl,
                ["payload"] = message.Payload,
                ["priority"] = message.Priority,
            };
            var doc = await RequestAsync(HttpMethod.Post, "/server/v1/messages", null, body, ct).ConfigureAwait(false);
            InvalidateIdentity(externalId);
            using (doc)
            {
                return doc.RootElement.TryGetProperty("id", out var id) ? id.GetString() ?? "" : "";
            }
        }

        /// <summary>Tracks analytics events for a player; returns how many the server accepted.</summary>
        public async Task<int> TrackAsync(string externalId, IEnumerable<Event> events, CancellationToken ct = default)
        {
            var list = events.ToList();
            var body = new Dictionary<string, object?> { ["external_id"] = externalId, ["events"] = list };
            var doc = await RequestAsync(HttpMethod.Post, "/server/v1/events", null, body, ct).ConfigureAwait(false);
            using (doc)
            {
                return doc.RootElement.TryGetProperty("accepted", out var n) && n.ValueKind == JsonValueKind.Number ? n.GetInt32() : list.Count;
            }
        }

        public Task<int> TrackAsync(string externalId, params Event[] events) => TrackAsync(externalId, events, CancellationToken.None);

        // ---- change stream ----------------------------------------------------------------------

        /// <summary>
        /// Keeps a WebSocket to <c>/server/v1/stream</c> open in the background (reconnecting with backoff) and
        /// invalidates the cache on every change, so console edits reach your servers within a second.
        /// </summary>
        public void StartStream(CancellationToken ct = default)
        {
            if (_stream != null) return;
            var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
            _stream = cts;
            _ = Task.Run(() => RunStreamLoopAsync(cts.Token), cts.Token);
        }

        public void StopStream()
        {
            var s = Interlocked.Exchange(ref _stream, null);
            s?.Cancel();
            s?.Dispose();
        }

        private async Task RunStreamLoopAsync(CancellationToken ct)
        {
            var backoff = TimeSpan.FromSeconds(1);
            while (!ct.IsCancellationRequested)
            {
                try
                {
                    await RunStreamOnceAsync(ct).ConfigureAwait(false);
                    backoff = TimeSpan.FromSeconds(1);
                }
                catch (OperationCanceledException) when (ct.IsCancellationRequested) { return; }
                catch (Exception e)
                {
                    _log("zanshin: change stream: " + e.Message + " (retry in " + backoff.TotalSeconds + "s)");
                }
                try { await Task.Delay(backoff, ct).ConfigureAwait(false); } catch (OperationCanceledException) { return; }
                backoff = TimeSpan.FromSeconds(Math.Min(backoff.TotalSeconds * 2, 30));
            }
        }

        private async Task RunStreamOnceAsync(CancellationToken ct)
        {
            using (var ws = new ClientWebSocket())
            {
                ws.Options.SetRequestHeader("x-api-key", _apiKey);
                var url = (_baseUrl.StartsWith("https") ? "wss" : "ws") + _baseUrl.Substring(_baseUrl.IndexOf(':')) + "/server/v1/stream";
                await ws.ConnectAsync(new Uri(url), ct).ConfigureAwait(false);
                Invalidate();
                _log("zanshin: change stream connected");
                using (var pingCts = CancellationTokenSource.CreateLinkedTokenSource(ct))
                {
                    var ping = PingLoopAsync(ws, pingCts.Token);
                    var buf = new byte[64 * 1024];
                    try
                    {
                        while (ws.State == WebSocketState.Open && !ct.IsCancellationRequested)
                        {
                            var text = await ReadFrameAsync(ws, buf, ct).ConfigureAwait(false);
                            if (text == null) break;
                            Change? ch;
                            try { ch = JsonSerializer.Deserialize<Change>(text, Json.Options); } catch (JsonException) { continue; }
                            if (ch == null) continue;
                            if (ch.Type == "change" || ch.Type == "resync")
                            {
                                Invalidate();
                                OnChange?.Invoke(ch);
                            }
                        }
                    }
                    finally
                    {
                        pingCts.Cancel();
                        try { await ping.ConfigureAwait(false); } catch (OperationCanceledException) { }
                    }
                }
            }
        }

        private static async Task PingLoopAsync(ClientWebSocket ws, CancellationToken ct)
        {
            var ping = Encoding.UTF8.GetBytes("{\"type\":\"ping\"}");
            while (!ct.IsCancellationRequested)
            {
                await Task.Delay(TimeSpan.FromSeconds(30), ct).ConfigureAwait(false);
                if (ws.State != WebSocketState.Open) return;
                await ws.SendAsync(new ArraySegment<byte>(ping), WebSocketMessageType.Text, true, ct).ConfigureAwait(false);
            }
        }

        private static async Task<string?> ReadFrameAsync(ClientWebSocket ws, byte[] buf, CancellationToken ct)
        {
            using (var ms = new System.IO.MemoryStream())
            {
                while (true)
                {
                    var r = await ws.ReceiveAsync(new ArraySegment<byte>(buf), ct).ConfigureAwait(false);
                    if (r.MessageType == WebSocketMessageType.Close) return null;
                    ms.Write(buf, 0, r.Count);
                    if (r.EndOfMessage) break;
                }
                return Encoding.UTF8.GetString(ms.ToArray());
            }
        }

        // ---- plumbing ---------------------------------------------------------------------------

        private static Dictionary<string, string> IdQuery(string externalId, IEnumerable<string>? names)
        {
            if (string.IsNullOrEmpty(externalId)) throw new ArgumentException("externalId is required", nameof(externalId));
            var q = new Dictionary<string, string> { ["external_id"] = externalId };
            if (names != null)
            {
                var joined = string.Join(",", names);
                if (joined.Length > 0) q["names"] = joined;
            }
            return q;
        }

        private async Task<string> CachedAsync(string kind, string externalId, Dictionary<string, string> query, CancellationToken ct, string? path = null)
        {
            var args = string.Join("&", query.OrderBy(kv => kv.Key).Select(kv => kv.Key + "=" + kv.Value));
            var key = kind + "\0" + externalId + "\0" + args;
            var gen = Interlocked.Read(ref _gen);
            if (_ttl > TimeSpan.Zero && _cache.TryGetValue(key, out var hit) && hit.Gen == gen && DateTime.UtcNow - hit.At < _ttl)
                return hit.Body;
            using (var doc = await RequestAsync(HttpMethod.Get, path ?? "/server/v1/" + kind, query, null, ct).ConfigureAwait(false))
            {
                var body = doc.RootElement.GetRawText();
                if (_ttl > TimeSpan.Zero) _cache[key] = new CacheEntry { At = DateTime.UtcNow, Gen = gen, Body = body };
                return body;
            }
        }

        private async Task<JsonDocument> RequestAsync(HttpMethod method, string path, Dictionary<string, string>? query, object? body, CancellationToken ct)
        {
            var url = _baseUrl + path;
            if (query != null && query.Count > 0)
                url += "?" + string.Join("&", query.Select(kv => Uri.EscapeDataString(kv.Key) + "=" + Uri.EscapeDataString(kv.Value)));
            using (var req = new HttpRequestMessage(method, url))
            {
                req.Headers.TryAddWithoutValidation("x-api-key", _apiKey);
                req.Headers.TryAddWithoutValidation("accept", "application/json");
                if (body != null)
                    req.Content = new StringContent(JsonSerializer.Serialize(body, Json.Options), Encoding.UTF8, "application/json");
                using (var resp = await _http.SendAsync(req, ct).ConfigureAwait(false))
                {
                    var text = resp.Content == null ? "" : await resp.Content.ReadAsStringAsync().ConfigureAwait(false);
                    if (!resp.IsSuccessStatusCode)
                    {
                        string code = "http_" + (int)resp.StatusCode, message = text;
                        try
                        {
                            using (var err = JsonDocument.Parse(text))
                            {
                                if (err.RootElement.TryGetProperty("code", out var c)) code = c.GetString() ?? code;
                                if (err.RootElement.TryGetProperty("message", out var m)) message = m.GetString() ?? message;
                            }
                        }
                        catch (JsonException) { }
                        throw new ZanshinException((int)resp.StatusCode, code, message);
                    }
                    return JsonDocument.Parse(string.IsNullOrWhiteSpace(text) ? "{}" : text);
                }
            }
        }

        private static T Parse<T>(JsonDocument doc)
        {
            using (doc) return Parse<T>(doc.RootElement.GetRawText());
        }

        private static T Parse<T>(string body) =>
            JsonSerializer.Deserialize<T>(body, Json.Options) ?? throw new ZanshinException(0, "decode", "empty response");

        private static IReadOnlyList<T> ParseList<T>(string body, string wrapper)
        {
            using (var doc = JsonDocument.Parse(body))
            {
                var root = doc.RootElement;
                var arr = root.ValueKind == JsonValueKind.Array ? root
                    : root.ValueKind == JsonValueKind.Object && root.TryGetProperty(wrapper, out var w) ? w
                    : default;
                if (arr.ValueKind != JsonValueKind.Array) return Array.Empty<T>();
                return arr.Deserialize<List<T>>(Json.Options) ?? new List<T>();
            }
        }

        public void Dispose() => StopStream();
    }
}
