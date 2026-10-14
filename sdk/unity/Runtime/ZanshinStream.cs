using System;
using System.Collections.Generic;
using System.Net.WebSockets;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Zanshin
{
    /// <summary>
    /// Live change stream over <c>/v1/stream</c> (System.Net.WebSockets; not available on WebGL — poll there).
    /// Events are raised on the receive task's thread: marshal to the main thread in Unity
    /// (e.g. enqueue and drain in Update, or use a SynchronizationContext).
    /// </summary>
    public sealed class ZanshinStream : IDisposable
    {
        public event Action<Flag> FlagChanged;
        public event Action<string> FlagDeleted;
        public event Action<List<Flag>> FlagsChanged;
        public event Action<List<Experiment>> ExperimentsChanged;
        public event Action<List<LiveEvent>> LiveEventsChanged;
        public event Action<List<InboxMessage>> InboxChanged;
        /// <summary>Every raw message as parsed JSON.</summary>
        public event Action<Dictionary<string, object>> MessageReceived;
        public event Action Connected;
        public event Action Disconnected;
        public event Action<Exception> Error;

        /// <summary>Latest flags by key, updated from pushes.</summary>
        public readonly Dictionary<string, Flag> Flags = new Dictionary<string, Flag>();
        public bool Reconnect = true;
        public bool IsConnected => _ws != null && _ws.State == WebSocketState.Open;

        readonly ZanshinClient _client;
        readonly string _names;
        ClientWebSocket _ws;
        CancellationTokenSource _cts;
        int _attempt;
        readonly Random _rng = new Random();

        public ZanshinStream(ZanshinClient client, IEnumerable<string> flagNames = null)
        {
            _client = client;
            _names = flagNames == null ? null : string.Join(",", flagNames);
        }

        /// <summary>Start (and keep) the connection; returns immediately.</summary>
        public void Connect()
        {
            Close();
            _cts = new CancellationTokenSource();
            _ = Run(_cts.Token);
        }

        Uri StreamUri()
        {
            var token = _client.Token ?? throw new InvalidOperationException("authenticate before connecting the stream");
            var b = _client.Url;
            if (b.StartsWith("https://")) b = "wss://" + b.Substring(8);
            else if (b.StartsWith("http://")) b = "ws://" + b.Substring(7);
            var q = "token=" + Uri.EscapeDataString(token);
            if (!string.IsNullOrEmpty(_names)) q += "&names=" + Uri.EscapeDataString(_names);
            return new Uri(b + "/v1/stream?" + q);
        }

        async Task Run(CancellationToken ct)
        {
            while (!ct.IsCancellationRequested)
            {
                try
                {
                    if (_client.Token == null) await _client.Refresh();
                    using (var ws = new ClientWebSocket())
                    {
                        _ws = ws;
                        await ws.ConnectAsync(StreamUri(), ct).ConfigureAwait(false);
                        _attempt = 0;
                        Connected?.Invoke();
                        var ping = PingLoop(ws, ct);
                        await Receive(ws, ct).ConfigureAwait(false);
                        _ws = null;
                        Disconnected?.Invoke();
                    }
                }
                catch (OperationCanceledException) { break; }
                catch (Exception e)
                {
                    _ws = null;
                    Error?.Invoke(e);
                }
                if (!Reconnect || ct.IsCancellationRequested) break;
                var delay = Math.Min(30000, 1000 * (1 << Math.Min(_attempt, 5))) * (0.5 + _rng.NextDouble());
                _attempt++;
                try { await Task.Delay((int)delay, ct).ConfigureAwait(false); } catch (OperationCanceledException) { break; }
            }
        }

        static async Task PingLoop(ClientWebSocket ws, CancellationToken ct)
        {
            var ping = Encoding.UTF8.GetBytes("{\"type\":\"ping\"}");
            try
            {
                while (ws.State == WebSocketState.Open && !ct.IsCancellationRequested)
                {
                    await Task.Delay(30000, ct).ConfigureAwait(false);
                    if (ws.State == WebSocketState.Open)
                        await ws.SendAsync(new ArraySegment<byte>(ping), WebSocketMessageType.Text, true, ct).ConfigureAwait(false);
                }
            }
            catch { /* socket closed */ }
        }

        async Task Receive(ClientWebSocket ws, CancellationToken ct)
        {
            var buf = new byte[16 * 1024];
            var sb = new StringBuilder();
            while (ws.State == WebSocketState.Open && !ct.IsCancellationRequested)
            {
                var r = await ws.ReceiveAsync(new ArraySegment<byte>(buf), ct).ConfigureAwait(false);
                if (r.MessageType == WebSocketMessageType.Close) return;
                sb.Append(Encoding.UTF8.GetString(buf, 0, r.Count));
                if (!r.EndOfMessage) continue;
                var text = sb.ToString();
                sb.Clear();
                Dictionary<string, object> msg;
                try { msg = Json.Obj(Json.Parse(text)); } catch { continue; }
                if (msg != null) Dispatch(msg);
            }
        }

        void Dispatch(Dictionary<string, object> msg)
        {
            switch (Json.Str(msg, "type"))
            {
                case "flag":
                {
                    var f = Flag.From(Json.Get(msg, "flag"));
                    lock (Flags) Flags[f.Key] = f;
                    FlagChanged?.Invoke(f);
                    break;
                }
                case "flag_deleted":
                {
                    var k = Json.Str(msg, "key");
                    lock (Flags) Flags.Remove(k);
                    FlagDeleted?.Invoke(k);
                    break;
                }
                case "flags":
                {
                    var fs = Lists.Map(Json.Get(msg, "flags"), Flag.From);
                    lock (Flags) foreach (var f in fs) Flags[f.Key] = f;
                    FlagsChanged?.Invoke(fs);
                    break;
                }
                case "experiments": ExperimentsChanged?.Invoke(Lists.Map(Json.Get(msg, "experiments"), Experiment.From)); break;
                case "live_events": LiveEventsChanged?.Invoke(Lists.Map(Json.Get(msg, "live_events"), LiveEvent.From)); break;
                case "inbox": InboxChanged?.Invoke(Lists.Map(Json.Get(msg, "messages"), InboxMessage.From)); break;
                case "error":
                    if (Json.Str(msg, "message", "").IndexOf("expired", StringComparison.OrdinalIgnoreCase) >= 0)
                        _ = _client.Refresh().ContinueWith(t => { if (t.IsFaulted) Error?.Invoke(t.Exception); });
                    break;
            }
            MessageReceived?.Invoke(msg);
        }

        /// <summary>Stop and do not reconnect.</summary>
        public void Close()
        {
            var cts = _cts;
            _cts = null;
            if (cts == null) return;
            cts.Cancel();
            var ws = _ws;
            _ws = null;
            if (ws != null && ws.State == WebSocketState.Open)
                _ = ws.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "bye", CancellationToken.None);
            cts.Dispose();
        }

        public void Dispose() => Close();
    }
}
