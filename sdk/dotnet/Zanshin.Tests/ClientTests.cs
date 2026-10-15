using System.Net;
using System.Text;
using OpenFeature.Constant;
using OpenFeature.Model;
using Zanshin.OpenFeature;
using Zanshin.Server;
using Reason = OpenFeature.Constant.Reason;
using Xunit;

namespace Zanshin.Tests;

/// <summary>Canned server: records requests, answers by path.</summary>
sealed class FakeServer : HttpMessageHandler
{
    public readonly List<HttpRequestMessage> Requests = new();
    public readonly List<string> Bodies = new();
    public Func<HttpRequestMessage, (HttpStatusCode, string)> Handler = _ => (HttpStatusCode.NotFound, "{\"code\":\"not_found\",\"message\":\"nope\"}");

    protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct)
    {
        Requests.Add(request);
        Bodies.Add(request.Content == null ? "" : await request.Content.ReadAsStringAsync(ct));
        var (status, body) = Handler(request);
        return new HttpResponseMessage(status) { Content = new StringContent(body, Encoding.UTF8, "application/json") };
    }
}

public class ClientTests
{
    const string FlagsBody = "{\"flags\":[{\"key\":\"welcome\",\"value\":true,\"value_type\":\"bool\",\"reason\":{\"kind\":\"rule\",\"rule_id\":\"r1\"},\"version\":3},{\"key\":\"price\",\"value\":9.5,\"value_type\":\"number\",\"variant\":\"b\",\"reason\":{\"kind\":\"experiment\",\"experiment\":\"pricing\"},\"version\":1}]}";

    static (ZanshinClient, FakeServer) Make(TimeSpan? ttl = null)
    {
        var fake = new FakeServer();
        var c = new ZanshinClient("http://zanshin.test/", "srv-key", new ZanshinOptions { Http = new HttpClient(fake), CacheTtl = ttl ?? TimeSpan.FromSeconds(30) });
        return (c, fake);
    }

    [Fact]
    public async Task Flags_are_parsed_sent_with_server_key_and_cached_until_invalidated()
    {
        var (c, fake) = Make();
        fake.Handler = r => r.RequestUri!.AbsolutePath == "/server/v1/flags" ? (HttpStatusCode.OK, FlagsBody) : (HttpStatusCode.NotFound, "{}");

        var flags = await c.FlagsAsync("player-1", new[] { "welcome", "price" });
        Assert.Equal(2, flags.Count);
        Assert.True(flags[0].AsBool());
        Assert.Equal("r1", flags[0].Reason.RuleId);
        Assert.Equal(9.5, flags[1].AsNumber());
        Assert.Equal("b", flags[1].Variant);
        Assert.Equal("srv-key", fake.Requests[0].Headers.GetValues("x-api-key").Single());
        Assert.Equal("?external_id=player-1&names=welcome%2Cprice", fake.Requests[0].RequestUri!.Query);

        await c.FlagsAsync("player-1", new[] { "welcome", "price" });
        Assert.Single(fake.Requests);                       // served from cache
        await c.FlagsAsync("player-2", new[] { "welcome", "price" });
        Assert.Equal(2, fake.Requests.Count);               // other player → own entry
        c.InvalidateIdentity("player-1");
        await c.FlagsAsync("player-1", new[] { "welcome", "price" });
        Assert.Equal(3, fake.Requests.Count);
        c.Invalidate();
        await c.FlagsAsync("player-2", new[] { "welcome", "price" });
        Assert.Equal(4, fake.Requests.Count);
    }

    [Fact]
    public async Task Errors_carry_status_and_code_and_typed_helpers_fall_back()
    {
        var (c, fake) = Make();
        var e = await Assert.ThrowsAsync<ZanshinException>(() => c.FlagAsync("p", "missing"));
        Assert.Equal(404, e.Status);
        Assert.Equal("not_found", e.Code);
        Assert.Equal("nope", e.Message);
        Assert.True(await c.FlagBoolAsync("p", "missing", fallback: true));
        Assert.Equal("x", await c.FlagStringAsync("p", "missing", "x"));
    }

    [Fact]
    public async Task Writes_post_json_and_invalidate_the_player()
    {
        var (c, fake) = Make();
        fake.Handler = r => r.RequestUri!.AbsolutePath switch
        {
            "/server/v1/flags" => (HttpStatusCode.OK, FlagsBody),
            "/server/v1/events" => (HttpStatusCode.OK, "{\"accepted\":2}"),
            "/server/v1/messages" => (HttpStatusCode.Created, "{\"id\":\"m1\"}"),
            _ => (HttpStatusCode.NotFound, "{}"),
        };
        await c.FlagsAsync("p1");
        var n = await c.TrackAsync("p1", new Event("level_up", "7", new Dictionary<string, object?> { ["zone"] = "forest" }), new Event("purchase", "4.99"));
        Assert.Equal(2, n);
        Assert.Contains("\"external_id\":\"p1\"", fake.Bodies[1]);
        Assert.Contains("\"metadata\":{\"zone\":\"forest\"}", fake.Bodies[1]);
        Assert.DoesNotContain("\"timestamp\"", fake.Bodies[1]);
        var id = await c.SendMessageAsync("p1", new DirectMessage { Title = "Hi", Body = "there", Payload = new { reward = 5 } });
        Assert.Equal("m1", id);
        Assert.Contains("\"payload\":{\"reward\":5}", fake.Bodies[2]);
        await c.FlagsAsync("p1");
        Assert.Equal(4, fake.Requests.Count);               // write invalidated p1's cache
    }
}

public class ProviderTests
{
    static (ZanshinProvider, FakeServer) Make()
    {
        var fake = new FakeServer();
        var c = new ZanshinClient("http://zanshin.test", "k", new ZanshinOptions { Http = new HttpClient(fake) });
        return (new ZanshinProvider(c), fake);
    }

    static string FlagJson(string value, string kind = "rule", string? variant = null) =>
        "{\"key\":\"f\",\"value\":" + value + ",\"value_type\":\"x\"," + (variant == null ? "" : "\"variant\":\"" + variant + "\",") + "\"reason\":{\"kind\":\"" + kind + "\",\"rule_id\":\"r9\"},\"version\":4}";

    [Fact]
    public async Task Resolves_typed_values_with_reason_variant_and_metadata()
    {
        var (p, fake) = Make();
        var ctx = EvaluationContext.Builder().SetTargetingKey("player-7").Build();

        fake.Handler = _ => (HttpStatusCode.OK, FlagJson("true"));
        var b = await p.ResolveBooleanValueAsync("f", false, ctx);
        Assert.True(b.Value);
        Assert.Equal(Reason.TargetingMatch, b.Reason);
        Assert.Equal(ErrorType.None, b.ErrorType);
        Assert.Equal(4, b.FlagMetadata!.GetInt("version"));
        Assert.Equal("r9", b.FlagMetadata!.GetString("rule_id"));
        Assert.Equal("/server/v1/flags/f", fake.Requests[0].RequestUri!.AbsolutePath);
        Assert.Equal("?external_id=player-7", fake.Requests[0].RequestUri!.Query);

        fake.Handler = _ => (HttpStatusCode.OK, FlagJson("\"blue\"", "experiment", "b"));
        var s = await p.ResolveStringValueAsync("g", "red", ctx);
        Assert.Equal("blue", s.Value);
        Assert.Equal(Reason.Split, s.Reason);
        Assert.Equal("b", s.Variant);

        fake.Handler = _ => (HttpStatusCode.OK, FlagJson("42", "default"));
        Assert.Equal(42, (await p.ResolveIntegerValueAsync("h", 1, ctx)).Value);
        Assert.Equal(Reason.Default, (await p.ResolveIntegerValueAsync("h", 1, ctx)).Reason);
        Assert.Equal(42.0, (await p.ResolveDoubleValueAsync("h", 1, ctx)).Value);

        fake.Handler = _ => (HttpStatusCode.OK, FlagJson("{\"hp\":10,\"tags\":[\"a\"],\"on\":false}", "disabled"));
        var st = await p.ResolveStructureValueAsync("o", new Value(), ctx);
        Assert.Equal(Reason.Disabled, st.Reason);
        Assert.Equal(10.0, st.Value.AsStructure!.GetValue("hp").AsDouble);
        Assert.Equal("a", st.Value.AsStructure!.GetValue("tags").AsList![0].AsString);
        Assert.False(st.Value.AsStructure!.GetValue("on").AsBoolean);
    }

    [Fact]
    public async Task Maps_errors_to_openfeature_error_types()
    {
        var (p, fake) = Make();
        var ctx = EvaluationContext.Builder().SetTargetingKey("p").Build();

        var missingKey = await p.ResolveBooleanValueAsync("f", true, EvaluationContext.Empty);
        Assert.True(missingKey.Value);
        Assert.Equal(ErrorType.TargetingKeyMissing, missingKey.ErrorType);
        Assert.Empty(fake.Requests);

        var notFound = await p.ResolveBooleanValueAsync("f", true, ctx);
        Assert.Equal(ErrorType.FlagNotFound, notFound.ErrorType);
        Assert.Equal(Reason.Error, notFound.Reason);

        fake.Handler = _ => (HttpStatusCode.OK, FlagJson("\"text\""));
        var mismatch = await p.ResolveBooleanValueAsync("f", true, ctx);
        Assert.True(mismatch.Value);
        Assert.Equal(ErrorType.TypeMismatch, mismatch.ErrorType);

        fake.Handler = _ => (HttpStatusCode.InternalServerError, "{\"code\":\"internal\",\"message\":\"boom\"}");
        var general = await p.ResolveStringValueAsync("other", "d", ctx);   // "f" is cached from the call above
        Assert.Equal("d", general.Value);
        Assert.Equal(ErrorType.General, general.ErrorType);
        Assert.Equal("boom", general.ErrorMessage);
    }
}
