// Unit tests for the C++ SDK with a scripted fake transport (no network).
#include <zanshin/zanshin.hpp>

#include <cassert>
#include <deque>
#include <iostream>

using namespace zanshin;

struct FakeTransport : HttpTransport {
    std::deque<HttpResponse> responses;
    std::vector<HttpRequest> calls;
    HttpResponse send(const HttpRequest& req) override {
        calls.push_back(req);
        if (responses.empty()) return HttpResponse{200, "{}", ""};
        HttpResponse r = responses.front();
        responses.pop_front();
        return r;
    }
};

static std::string header(const HttpRequest& r, const std::string& name) {
    for (const auto& h : r.headers) if (h.first == name) return h.second;
    return "";
}

static void test_json() {
    Json j = Json::parse(R"({"a": [1, 2.5, "x\u00e9\n", true, null], "b": {"c": -3e2}, "d": "\ud83d\ude00"})");
    assert(j["a"].size() == 5);
    assert(j["a"][0].as_int() == 1 && j["a"][1].as_number() == 2.5);
    assert(j["a"][2].as_string() == "x\xC3\xA9\n");
    assert(j["a"][3].as_bool() && j["a"][4].is_null());
    assert(j["b"]["c"].as_number() == -300);
    assert(j["d"].as_string() == "\xF0\x9F\x98\x80");
    assert(j["missing"]["deep"].is_null());
    Json o = Json{{"k", "v"}, {"n", 2}, {"arr", Json::Array{1, "two"}}};
    assert(o.dump() == R"({"arr":[1,"two"],"k":"v","n":2})");
    assert(Json::parse(o.dump()).dump() == o.dump());
    bool ok = true;
    Json::try_parse("{bad", ok);
    assert(!ok);
    assert(Json("true").as_bool() && Json("2.5").as_number() == 2.5 && Json("abc").as_number(7) == 7);
}

static void test_client() {
    auto t = std::make_shared<FakeTransport>();
    std::vector<std::pair<std::string, std::string>> sessions;
    ClientOptions o;
    o.url = "https://z.example/";
    o.api_key = "zk_c_test";
    o.on_session = [&](const std::string& a, const std::string& b) { sessions.emplace_back(a, b); };
    Client z(o, t);

    // session-auth call without a session fails locally
    auto f0 = z.flags();
    assert(!f0 && f0.error().status == 401 && t->calls.empty());

    const std::string hdr = "eyJhbGciOiJIUzI1NiJ9";       // {"alg":"HS256"}
    const std::string far = "eyJleHAiOjQxMDI0NDQ4MDB9";   // {"exp":4102444800}
    const std::string tok = hdr + "." + far + ".sig";
    t->responses.push_back({200, R"({"token":")" + tok + R"(","refresh_token":"r1","identity":{"id":"i1","external_id":"p1"},"properties":{"default":{},"computed":{},"custom":{"level":3}}})", ""});
    auto s = z.authenticate("p1", Json{{"platform", "ios"}}, Json{{"level", 3}});
    assert(s && s->identity.external_id == "p1" && s->properties.custom["level"].as_int() == 3);
    assert(t->calls.size() == 1 && t->calls[0].method == "POST" && t->calls[0].url == "https://z.example/v1/authenticate");
    assert(header(t->calls[0], "x-api-key") == "zk_c_test");
    assert(Json::parse(t->calls[0].body)["custom"]["level"].as_int() == 3);
    assert(z.is_authenticated() && sessions.size() == 1 && sessions[0].first == tok);

    // flags: bearer header, names query, typed helpers
    t->responses.push_back({200, R"({"flags":[{"key":"a","value":"true","value_type":"bool","reason":{"kind":"default"},"version":2}]})", ""});
    auto fl = z.flags({"a", "b c"});
    assert(fl && fl->size() == 1 && (*fl)[0].key == "a" && (*fl)[0].value.as_bool() && (*fl)[0].reason == "default");
    assert(header(t->calls.back(), "Authorization") == "Bearer " + tok);
    assert(t->calls.back().url == "https://z.example/v1/flags?names=a%2Cb%20c");
    t->responses.push_back({200, R"({"key":"n","value":"2.5","value_type":"number"})", ""});
    assert(z.flag_number("n", 1) == 2.5);
    t->responses.push_back({404, R"({"code":"not_found","message":"no flag"})", ""});
    assert(z.flag_bool("missing", true) == true);
    assert(z.is_authenticated());  // 404 keeps the session
    t->responses.push_back({200, R"({"key":"j","value":{"x":1},"value_type":"json"})", ""});
    assert(z.flag_string("j", "") == R"({"x":1})");

    // track: value + metadata + timestamp
    t->responses.push_back({200, R"({"accepted":1,"rejected":0})", ""});
    auto n = z.track("level_completed", std::string("3"), Json{{"stars", 2}});
    assert(n && *n == 1);
    Json ev = Json::parse(t->calls.back().body)["events"][0];
    assert(ev["name"].as_string() == "level_completed" && ev["value"].as_string() == "3" && ev["metadata"]["stars"].as_int() == 2);
    assert(ev["timestamp"].as_string().back() == 'Z');

    // live events + inbox + experiments shapes
    t->responses.push_back({200, R"({"live_events":[{"key":"w","name":"W","active":true,"current":{"start":"s","end":"e"}}]})", ""});
    auto le = z.live_events({}, true);
    assert(le && (*le)[0].active && (*le)[0].current.present && !(*le)[0].next.present);
    assert(t->calls.back().url == "https://z.example/v1/live-events?upcoming=true");
    t->responses.push_back({200, R"({"left":false})", ""});
    assert(z.leave_live_event("w").value_or(true) == false);
    t->responses.push_back({200, R"({"messages":[{"id":"m1","title":"Hi","read_at":null,"payload":{"gold":5}}]})", ""});
    auto ib = z.inbox(10);
    assert(ib && (*ib)[0].id == "m1" && !(*ib)[0].read() && (*ib)[0].payload["gold"].as_int() == 5);
    assert(t->calls.back().url == "https://z.example/v1/messages?limit=10");

    // expired token → refresh first, then the call
    const std::string expired = hdr + ".eyJleHAiOjF9.sig";  // {"exp":1}
    t->responses.push_back({200, R"({"token":")" + tok + R"(","refresh_token":"r2","identity":{"id":"i1"},"properties":{}})", ""});
    assert(z.restore(expired, "r1"));
    assert(t->calls.back().url == "https://z.example/v1/authenticate/refresh" && Json::parse(t->calls.back().body)["refresh_token"].as_string() == "r1");
    assert(z.refresh_token() == "r2");
    t->responses.push_back({200, R"({"audiences":["whales"]})", ""});
    auto au = z.audiences();
    assert(au && (*au)[0] == "whales");

    // 401 on a session call clears the session
    t->responses.push_back({401, R"({"code":"unauthenticated","message":"revoked"})", ""});
    auto id = z.identity();
    assert(!id && id.error().code == "unauthenticated" && !z.is_authenticated() && sessions.back().first.empty());

    // transport failure
    t->responses.push_back({200, R"({"token":")" + tok + R"(","refresh_token":"r3","identity":{"id":"i1"},"properties":{}})", ""});
    assert(z.authenticate("p1"));
    t->responses.push_back({0, "", "dns failure"});
    auto p = z.properties();
    assert(!p && p.error().status == 0 && p.error().message == "dns failure");

    // logout revokes with the API key
    t->responses.push_back({200, "{}", ""});
    assert(z.logout());
    assert(t->calls.back().url == "https://z.example/v1/authenticate/logout" && header(t->calls.back(), "x-api-key") == "zk_c_test");
    assert(!z.is_authenticated());
}

int main() {
    test_json();
    test_client();
    std::cout << "cpp sdk tests OK" << std::endl;
    return 0;
}
