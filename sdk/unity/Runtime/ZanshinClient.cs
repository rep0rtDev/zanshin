using System;
using System.Collections.Generic;
using System.Net.Http;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Zanshin
{
    /// <summary>
    /// Zanshin client for one identity (player / device). Thread-safe; all calls are async Tasks.
    /// <code>
    /// var z = new ZanshinClient("https://liveops.example.com", CLIENT_KEY);
    /// await z.Authenticate(SystemInfo.deviceUniqueIdentifier, new Dictionary&lt;string, object&gt; { ["platform"] = "ios" });
    /// bool shopV2 = (await z.Flag("shop_v2")).AsBool();
    /// </code>
    /// </summary>
    public sealed class ZanshinClient : IDisposable
    {
        public string Url { get; }
        public string ApiKey { get; }
        /// <summary>Fires on every token rotation (persist it) and with <c>null</c> on logout / revoked session.</summary>
        public event Action<Session> SessionChanged;
        /// <summary>Refresh this many seconds before the access token expires.</summary>
        public int RefreshLeewaySeconds = 60;

        readonly HttpClient _http;
        readonly object _lock = new object();
        string _token, _refreshToken;
        long _expiresAt; // unix seconds, 0 = unknown
        Task<Session> _refreshing;

        public ZanshinClient(string url, string apiKey, HttpClient http = null)
        {
            Url = url.TrimEnd('/');
            ApiKey = apiKey;
            _http = http ?? new HttpClient { Timeout = TimeSpan.FromSeconds(15) };
        }

        public string Token { get { lock (_lock) return _token; } }
        public string RefreshToken { get { lock (_lock) return _refreshToken; } }
        public bool IsAuthenticated => Token != null;

        /// <summary>Resume a persisted session; refreshes right away if the access token is stale.</summary>
        public async Task Restore(string token, string refreshToken)
        {
            SetTokens(token, refreshToken);
            if (ExpiringSoon()) await Refresh();
        }

        void SetTokens(string token, string refreshToken)
        {
            lock (_lock)
            {
                _token = token;
                _refreshToken = refreshToken;
                _expiresAt = DecodeExp(token);
            }
        }

        void ClearTokens()
        {
            lock (_lock) { _token = null; _refreshToken = null; _expiresAt = 0; }
            SessionChanged?.Invoke(null);
        }

        bool ExpiringSoon()
        {
            lock (_lock)
            {
                if (_token == null) return true;
                if (_expiresAt == 0) return false;
                return DateTimeOffset.UtcNow.ToUnixTimeSeconds() >= _expiresAt - RefreshLeewaySeconds;
            }
        }

        static long DecodeExp(string jwt)
        {
            try
            {
                var parts = jwt.Split('.');
                if (parts.Length < 2) return 0;
                var b64 = parts[1].Replace('-', '+').Replace('_', '/');
                while (b64.Length % 4 != 0) b64 += "=";
                var payload = Json.Parse(Encoding.UTF8.GetString(Convert.FromBase64String(b64)));
                return Json.Int(payload, "exp");
            }
            catch { return 0; }
        }

        // ---- transport ------------------------------------------------------------------------

        enum Auth { Key, Session }

        async Task<object> Request(HttpMethod method, string path, object body = null, Auth auth = Auth.Session, string query = null, CancellationToken ct = default)
        {
            var url = Url + path + (string.IsNullOrEmpty(query) ? "" : "?" + query);
            using (var req = new HttpRequestMessage(method, url))
            {
                req.Headers.TryAddWithoutValidation("accept", "application/json");
                if (auth == Auth.Key) req.Headers.TryAddWithoutValidation("x-api-key", ApiKey);
                else
                {
                    if (!IsAuthenticated) throw new ZanshinException(401, "unauthenticated", "not authenticated");
                    if (ExpiringSoon() && RefreshToken != null) await Refresh();
                    req.Headers.TryAddWithoutValidation("authorization", "Bearer " + Token);
                }
                if (body != null) req.Content = new StringContent(Json.Serialize(body), Encoding.UTF8, "application/json");
                using (var res = await _http.SendAsync(req, ct).ConfigureAwait(false))
                {
                    var text = res.Content == null ? "" : await res.Content.ReadAsStringAsync().ConfigureAwait(false);
                    if (!res.IsSuccessStatusCode)
                    {
                        string code = res.ReasonPhrase ?? "error", message = text;
                        try
                        {
                            var e = Json.Parse(text);
                            code = Json.Str(e, "code", code);
                            message = Json.Str(e, "message", message);
                        }
                        catch { /* non-JSON error */ }
                        if ((int)res.StatusCode == 401 && auth == Auth.Session) ClearTokens();
                        throw new ZanshinException((int)res.StatusCode, code, message);
                    }
                    return text.Length == 0 ? new Dictionary<string, object>() : Json.Parse(text);
                }
            }
        }

        static string Names(IEnumerable<string> names) => names == null ? null : "names=" + Uri.EscapeDataString(string.Join(",", names));

        // ---- session --------------------------------------------------------------------------

        /// <summary>Authenticate (create or resume) identity <paramref name="id"/> and open a session.</summary>
        public async Task<Session> Authenticate(string id, Dictionary<string, object> defaultProps = null, Dictionary<string, object> customProps = null)
        {
            var body = new Dictionary<string, object>
            {
                ["id"] = id,
                ["default"] = defaultProps ?? new Dictionary<string, object>(),
                ["custom"] = customProps ?? new Dictionary<string, object>()
            };
            var s = Session.From(await Request(HttpMethod.Post, "/v1/authenticate", body, Auth.Key));
            SetTokens(s.Token, s.RefreshToken);
            SessionChanged?.Invoke(s);
            return s;
        }

        /// <summary>Rotate tokens. Concurrent callers share a single request.</summary>
        public Task<Session> Refresh()
        {
            lock (_lock)
            {
                if (_refreshing != null) return _refreshing;
                if (_refreshToken == null) return Task.FromException<Session>(new ZanshinException(401, "unauthenticated", "no refresh token"));
                _refreshing = DoRefresh(_refreshToken);
                return _refreshing;
            }
        }

        async Task<Session> DoRefresh(string refreshToken)
        {
            try
            {
                var s = Session.From(await Request(HttpMethod.Post, "/v1/authenticate/refresh", new Dictionary<string, object> { ["refresh_token"] = refreshToken }, Auth.Key));
                SetTokens(s.Token, s.RefreshToken);
                SessionChanged?.Invoke(s);
                return s;
            }
            catch (ZanshinException e) when (e.Status == 401)
            {
                ClearTokens();
                throw;
            }
            finally
            {
                lock (_lock) _refreshing = null;
            }
        }

        /// <summary>Revoke the session server-side and forget tokens.</summary>
        public async Task Logout()
        {
            string t, rt;
            lock (_lock) { t = _token; rt = _refreshToken; }
            ClearTokens();
            if (t == null) return;
            await Request(HttpMethod.Post, "/v1/authenticate/logout", new Dictionary<string, object> { ["token"] = t, ["refresh_token"] = rt }, Auth.Key);
        }

        // ---- identity & properties ------------------------------------------------------------

        public async Task<Identity> GetIdentity() => Identity.From(await Request(HttpMethod.Get, "/v1/identity"));

        public async Task DeleteIdentity()
        {
            await Request(HttpMethod.Delete, "/v1/identity");
            ClearTokens();
        }

        public async Task<Properties> GetProperties() => Properties.From(await Request(HttpMethod.Get, "/v1/properties"));

        /// <summary>Merge properties; a <c>null</c> value removes the key.</summary>
        public async Task<Properties> UpdateProperties(Dictionary<string, object> defaultProps = null, Dictionary<string, object> customProps = null)
        {
            var body = new Dictionary<string, object>
            {
                ["default"] = defaultProps ?? new Dictionary<string, object>(),
                ["custom"] = customProps ?? new Dictionary<string, object>()
            };
            return Properties.From(await Request(HttpMethod.Put, "/v1/properties", body));
        }

        // ---- flags ----------------------------------------------------------------------------

        public async Task<List<Flag>> Flags(IEnumerable<string> names = null) =>
            Lists.Map(Json.Get(await Request(HttpMethod.Get, "/v1/flags", null, Auth.Session, Names(names)), "flags"), Zanshin.Flag.From);

        public async Task<Flag> Flag(string key) => Zanshin.Flag.From(await Request(HttpMethod.Get, "/v1/flags/" + Uri.EscapeDataString(key)));

        /// <summary>Flag value or <paramref name="fallback"/> when the flag does not exist.</summary>
        public async Task<bool> FlagBool(string key, bool fallback = false)
        {
            try { return (await Flag(key)).AsBool(fallback); }
            catch (ZanshinException e) when (e.IsNotFound) { return fallback; }
        }

        public async Task<string> FlagString(string key, string fallback = null)
        {
            try { return (await Flag(key)).AsString(fallback); }
            catch (ZanshinException e) when (e.IsNotFound) { return fallback; }
        }

        public async Task<double> FlagNumber(string key, double fallback = 0)
        {
            try { return (await Flag(key)).AsNumber(fallback); }
            catch (ZanshinException e) when (e.IsNotFound) { return fallback; }
        }

        public async Task<List<string>> Audiences() => Json.Strings(await Request(HttpMethod.Get, "/v1/audiences"), "audiences");

        // ---- experiments ----------------------------------------------------------------------

        public async Task<List<Experiment>> Experiments(IEnumerable<string> names = null) =>
            Lists.Map(Json.Get(await Request(HttpMethod.Get, "/v1/experiments", null, Auth.Session, Names(names)), "experiments"), Experiment.From);

        /// <summary>Record that the identity actually saw its variant.</summary>
        public Task Expose(string experiment) => Request(HttpMethod.Post, "/v1/experiments/" + Uri.EscapeDataString(experiment) + "/expose");

        // ---- live events ----------------------------------------------------------------------

        public async Task<List<LiveEvent>> LiveEvents(IEnumerable<string> names = null, bool upcoming = false)
        {
            var q = Names(names);
            if (upcoming) q = (q == null ? "" : q + "&") + "upcoming=true";
            return Lists.Map(Json.Get(await Request(HttpMethod.Get, "/v1/live-events", null, Auth.Session, q), "live_events"), LiveEvent.From);
        }

        // ---- inbox ----------------------------------------------------------------------------

        public async Task<List<InboxMessage>> Inbox(int? limit = null) =>
            Lists.Map(Json.Get(await Request(HttpMethod.Get, "/v1/messages", null, Auth.Session, limit.HasValue ? "limit=" + limit.Value : null), "messages"), InboxMessage.From);

        public async Task<InboxMessage> MarkRead(string id) =>
            InboxMessage.From(await Request(HttpMethod.Put, "/v1/messages/" + Uri.EscapeDataString(id), new Dictionary<string, object> { ["read"] = true }));

        /// <summary>Consume (remove) a message; it is never redelivered.</summary>
        public Task Consume(string id) => Request(HttpMethod.Delete, "/v1/messages/" + Uri.EscapeDataString(id));

        // ---- push tokens ----------------------------------------------------------------------

        /// <summary>Register this device for push: <paramref name="platform"/> is "fcm" or "apns".</summary>
        public async Task<PushToken> RegisterPushToken(string platform, string token, string deviceId = "") =>
            PushToken.From(await Request(HttpMethod.Put, "/v1/push-tokens", new Dictionary<string, object> { ["platform"] = platform, ["token"] = token, ["device_id"] = deviceId ?? "" }));

        public async Task<List<PushToken>> PushTokens() => Lists.Map(Json.Get(await Request(HttpMethod.Get, "/v1/push-tokens"), "tokens"), PushToken.From);

        public Task UnregisterPushToken(string token) => Request(HttpMethod.Delete, "/v1/push-tokens/" + Uri.EscapeDataString(token));

        // ---- analytics ------------------------------------------------------------------------

        /// <summary>Report events; returns how many were accepted.</summary>
        public async Task<long> Track(params Event[] events)
        {
            if (events == null || events.Length == 0) return 0;
            var list = new List<object>();
            foreach (var e in events) list.Add(e.ToJson());
            return Json.Int(await Request(HttpMethod.Post, "/v1/events", new Dictionary<string, object> { ["events"] = list }), "accepted");
        }

        public Task<long> Track(string name, object value = null, Dictionary<string, object> metadata = null) =>
            Track(new Event(name, value == null ? null : value is string s ? s : Json.Serialize(value), metadata));

        /// <summary>Open a live change stream for this session (connect it with <see cref="ZanshinStream.Connect"/>).</summary>
        public ZanshinStream Stream(IEnumerable<string> flagNames = null) => new ZanshinStream(this, flagNames);

        public void Dispose() => _http.Dispose();
    }
}
