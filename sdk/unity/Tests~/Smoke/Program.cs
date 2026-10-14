// Smoke test against a live server: ZANSHIN_TEST_URL, ZANSHIN_TEST_CLIENT_KEY, optional ZANSHIN_TEST_ADMIN_KEY.
using System;
using System.Collections.Generic;
using System.Net.Http;
using System.Text;
using System.Threading.Tasks;
using Zanshin;

static class Program
{
    static void Check(bool ok, string what) { if (!ok) throw new Exception("FAILED: " + what); Console.WriteLine("ok " + what); }

    static async Task<int> Main()
    {
        var url = Environment.GetEnvironmentVariable("ZANSHIN_TEST_URL");
        var key = Environment.GetEnvironmentVariable("ZANSHIN_TEST_CLIENT_KEY");
        var admin = Environment.GetEnvironmentVariable("ZANSHIN_TEST_ADMIN_KEY");
        if (url == null || key == null) { Console.WriteLine("skipped (no ZANSHIN_TEST_URL)"); return 0; }

        var sessions = new List<Session>();
        var z = new ZanshinClient(url, key);
        z.SessionChanged += s => sessions.Add(s);
        var id = "unity-sdk-" + DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        var s = await z.Authenticate(id, new Dictionary<string, object> { ["platform"] = "unity" }, new Dictionary<string, object> { ["nick"] = "Neo" });
        Check(s.Identity.ExternalId == id && z.IsAuthenticated && sessions.Count == 1, "authenticate");

        var props = await z.UpdateProperties(null, new Dictionary<string, object> { ["level"] = 3 });
        Check(Json.Int(props.Custom, "level") == 3 && Json.Str(props.Custom, "nick") == "Neo", "properties merge");

        var flags = await z.Flags();
        Check(flags != null, "flags list");
        try { await z.Flag("definitely_missing_flag_unity"); Check(false, "missing flag throws"); }
        catch (ZanshinException e) { Check(e.IsNotFound, "missing flag 404"); }
        Check(await z.FlagBool("definitely_missing_flag_unity", true), "FlagBool fallback");
        Check((await z.Audiences()) != null && (await z.Experiments()) != null && (await z.LiveEvents(null, true)) != null && (await z.Inbox()) != null, "reads");
        Check(await z.Track(new Event("unity.ping", "1", new Dictionary<string, object> { ["k"] = "v" }), new Event("unity.pong")) == 2, "track 2");
        Check(await z.Track("unity.level_up", 7) == 1, "track value");
        try { await z.Track("$reserved"); Check(false, "reserved event throws"); }
        catch (ZanshinException e) { Check(e.Status == 400, "reserved event 400"); }

        var pt = await z.RegisterPushToken("fcm", "unity-token-" + id, "dev-1");
        Check(pt.Platform == "fcm" && (await z.PushTokens()).Count == 1, "push token");
        await z.UnregisterPushToken(pt.Token);
        Check((await z.PushTokens()).Count == 0, "push token removed");

        var before = z.Token;
        await z.Refresh();
        Check(z.Token != before && sessions.Count == 2, "refresh rotates");

        var stream = z.Stream();
        var hello = new TaskCompletionSource<bool>();
        stream.MessageReceived += m => { if (Json.Str(m, "type") == "hello") hello.TrySetResult(true); };
        var errors = new List<Exception>();
        stream.Error += e => errors.Add(e);
        stream.Connect();
        Check(await Task.WhenAny(hello.Task, Task.Delay(5000)) == hello.Task, "stream hello");
        Check(stream.IsConnected, "stream connected");

        if (admin != null)
        {
            var fkey = "unity_sdk_" + DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
            var pushed = new TaskCompletionSource<Flag>();
            stream.FlagChanged += f => { if (f.Key == fkey) pushed.TrySetResult(f); };
            var http = new HttpClient();
            http.DefaultRequestHeaders.Add("x-api-key", admin);
            var res = await http.PostAsync(url + "/admin/v1/flags", new StringContent(Json.Serialize(new Dictionary<string, object> { ["key"] = fkey, ["value_type"] = "bool", ["default_value"] = true }), Encoding.UTF8, "application/json"));
            Check((int)res.StatusCode == 201, "admin create flag");
            Check(await Task.WhenAny(pushed.Task, Task.Delay(5000)) == pushed.Task && pushed.Task.Result.AsBool(), "flag pushed over stream");
            Check(stream.Flags.ContainsKey(fkey), "stream cache");
            var deleted = new TaskCompletionSource<string>();
            stream.FlagDeleted += k => { if (k == fkey) deleted.TrySetResult(k); };
            await http.DeleteAsync(url + "/admin/v1/flags/" + fkey);
            Check(await Task.WhenAny(deleted.Task, Task.Delay(5000)) == deleted.Task && !stream.Flags.ContainsKey(fkey), "flag delete pushed");
        }
        stream.Close();

        await z.Logout();
        Check(!z.IsAuthenticated && sessions[sessions.Count - 1] == null, "logout");
        try { await z.Flags(); Check(false, "unauthenticated throws"); }
        catch (ZanshinException e) { Check(e.IsUnauthenticated, "unauthenticated 401"); }
        Console.WriteLine("ALL OK" + (errors.Count > 0 ? " (stream errors: " + errors.Count + ")" : ""));
        return 0;
    }
}
