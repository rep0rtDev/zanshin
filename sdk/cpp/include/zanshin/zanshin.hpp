// Zanshin C++17 client SDK — header-only, dependency-free. Bring your own HTTP transport
// (libcurl adapter in zanshin/curl_transport.hpp; Cocos2d-x, Unreal, SDL_net … adapters are a few lines).
//
//   #include <zanshin/zanshin.hpp>
//   #include <zanshin/curl_transport.hpp>
//   zanshin::Client z({"https://liveops.example.com", "zk_c_..."}, std::make_shared<zanshin::CurlTransport>());
//   auto s = z.authenticate("player-42", {{"platform", "ios"}});
//   if (s) { bool shop = z.flag_bool("new_shop", false); z.track("level_completed", "3", {{"stars", 2}}); }
#pragma once
#include <chrono>
#include <ctime>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <vector>

#include "json.hpp"

namespace zanshin {

struct HttpRequest {
    std::string method;
    std::string url;
    std::vector<std::pair<std::string, std::string>> headers;
    std::string body;
    int timeout_ms = 30000;
};

struct HttpResponse {
    int status = 0;             ///< 0 = transport failure (see error)
    std::string body;
    std::string error;
};

/// Synchronous transport. Implementations must be safe to call from any thread.
class HttpTransport {
public:
    virtual ~HttpTransport() = default;
    virtual HttpResponse send(const HttpRequest& req) = 0;
};

struct Error {
    int status = 0;             ///< HTTP status, 0 for transport errors
    std::string code;
    std::string message;
    bool is_not_found() const { return status == 404; }
    bool is_unauthenticated() const { return status == 401; }
    bool is_rate_limited() const { return status == 429; }
    std::string to_string() const { return std::to_string(status) + " " + code + ": " + message; }
};

/// Result of a call: either a value or an error.
template <typename T>
class Result {
public:
    Result(T v) : value_(std::move(v)) {}
    Result(Error e) : error_(std::move(e)) {}
    explicit operator bool() const { return !error_.has_value(); }
    bool ok() const { return !error_.has_value(); }
    const T& value() const { return *value_; }
    T& value() { return *value_; }
    const T* operator->() const { return &*value_; }
    const T& operator*() const { return *value_; }
    const Error& error() const { return *error_; }
    T value_or(T fallback) const { return value_ ? *value_ : std::move(fallback); }

private:
    std::optional<T> value_;
    std::optional<Error> error_;
};

struct Identity {
    std::string id, external_id, created_at, updated_at, last_seen_at;
    static Identity from(const Json& j) {
        return {j["id"].as_string(), j["external_id"].as_string(), j["created_at"].as_string(), j["updated_at"].as_string(), j["last_seen_at"].as_string()};
    }
};

struct Properties {
    Json defaults, computed, custom;
    static Properties from(const Json& j) { return {j["default"], j["computed"], j["custom"]}; }
};

struct Session {
    std::string token, refresh_token;
    Identity identity;
    Properties properties;
    static Session from(const Json& j) {
        return {j["token"].as_string(), j["refresh_token"].as_string(), Identity::from(j["identity"]), Properties::from(j["properties"])};
    }
};

struct Flag {
    std::string key, value_type, variant, reason;
    Json value;
    long long version = 0;
    Json raw;
    static Flag from(const Json& j) {
        return {j["key"].as_string(), j["value_type"].as_string(), j["variant"].as_string(), j["reason"]["kind"].as_string(), j["value"], j["version"].as_int(), j};
    }
};

struct Experiment {
    std::string key, name, variant;
    Json value;
    long long version = 0;
    static Experiment from(const Json& j) { return {j["key"].as_string(), j["name"].as_string(), j["variant"].as_string(), j["value"], j["version"].as_int()}; }
};

struct Occurrence {
    std::string start, end;
    bool present = false;
    static Occurrence from(const Json& j) { return {j["start"].as_string(), j["end"].as_string(), j.is_object()}; }
};

struct LiveEvent {
    std::string key, name, description;
    Json value;
    bool active = false;
    Occurrence current, next;
    long long version = 0;
    static LiveEvent from(const Json& j) {
        return {j["key"].as_string(), j["name"].as_string(), j["description"].as_string(), j["value"], j["active"].as_bool(), Occurrence::from(j["current"]), Occurrence::from(j["next"]), j["version"].as_int()};
    }
};

struct InboxMessage {
    std::string id, key, title, body, image_url, delivered_at, read_at, expires_at;
    Json payload;
    long long priority = 0;
    bool read() const { return !read_at.empty(); }
    static InboxMessage from(const Json& j) {
        return {j["id"].as_string(), j["key"].as_string(), j["title"].as_string(), j["body"].as_string(), j["image_url"].as_string(), j["delivered_at"].as_string(), j["read_at"].as_string(), j["expires_at"].as_string(), j["payload"], j["priority"].as_int()};
    }
};

struct PushToken {
    std::string platform, token, device_id;
    long long failures = 0;
    static PushToken from(const Json& j) { return {j["platform"].as_string(), j["token"].as_string(), j["device_id"].as_string(), j["failures"].as_int()}; }
};

struct EventInput {
    std::string name;
    std::optional<std::string> value;
    Json metadata;                      ///< object or null
    std::optional<std::string> timestamp;   ///< ISO-8601; empty = now
    std::optional<std::string> id;      ///< idempotency key (UUID)
};

struct ClientOptions {
    std::string url;
    std::string api_key;                ///< client API key (zk_c_…)
    int refresh_leeway_seconds = 60;
    std::string user_agent = "zanshin-cpp/0.1";
    /// Called with the new tokens after authenticate/refresh/identify, and with empty strings on logout / revocation.
    std::function<void(const std::string& token, const std::string& refresh_token)> on_session;
};

class Client {
public:
    Client(ClientOptions opts, std::shared_ptr<HttpTransport> transport)
        : opts_(std::move(opts)), http_(std::move(transport)) {
        while (!opts_.url.empty() && opts_.url.back() == '/') opts_.url.pop_back();
    }

    const std::string& url() const { return opts_.url; }
    std::string token() const { std::lock_guard<std::mutex> g(m_); return token_; }
    std::string refresh_token() const { std::lock_guard<std::mutex> g(m_); return refresh_token_; }
    bool is_authenticated() const { std::lock_guard<std::mutex> g(m_); return !token_.empty(); }

    /// Resume a persisted session; refreshes right away when the access token is stale.
    Result<bool> restore(const std::string& token, const std::string& refresh_token) {
        { std::lock_guard<std::mutex> g(m_); token_ = token; refresh_token_ = refresh_token; expires_at_ = jwt_exp(token); }
        if (expiring_soon()) { auto r = refresh(); if (!r) return r.error(); }
        return true;
    }

    // ---- session -------------------------------------------------------------------------------

    Result<Session> authenticate(const std::string& id, Json default_props = Json::object(), Json custom_props = Json::object()) {
        Json body = Json::object();
        body["id"] = id;
        body["default"] = default_props.is_object() ? default_props : Json::object();
        body["custom"] = custom_props.is_object() ? custom_props : Json::object();
        auto r = request("POST", "/v1/authenticate", body, Auth::Key);
        if (!r) return r.error();
        Session s = Session::from(*r);
        set_session(s);
        return s;
    }

    Result<Session> refresh() {
        std::string rt;
        { std::lock_guard<std::mutex> g(m_); rt = refresh_token_; }
        if (rt.empty()) return Error{401, "unauthenticated", "no refresh token"};
        std::lock_guard<std::mutex> rg(refresh_m_);
        { std::lock_guard<std::mutex> g(m_); if (refresh_token_ != rt) {
            // another thread refreshed meanwhile
            return Session{token_, refresh_token_, {}, {}}; } }
        auto r = request("POST", "/v1/authenticate/refresh", Json{{"refresh_token", rt}}, Auth::Key);
        if (!r) { if (r.error().status == 401) clear_session(); return r.error(); }
        Session s = Session::from(*r);
        set_session(s);
        return s;
    }

    Result<bool> logout() {
        std::string t, rt;
        { std::lock_guard<std::mutex> g(m_); t = token_; rt = refresh_token_; }
        clear_session();
        if (t.empty()) return true;
        auto r = request("POST", "/v1/authenticate/logout", Json{{"token", t}, {"refresh_token", rt}}, Auth::Key);
        if (!r) return r.error();
        return true;
    }

    // ---- identity & properties -----------------------------------------------------------------

    Result<Identity> identity() { return map<Identity>(request("GET", "/v1/identity"), Identity::from); }
    Result<bool> delete_identity() {
        auto r = request("DELETE", "/v1/identity");
        if (!r) return r.error();
        clear_session();
        return true;
    }
    Result<Properties> properties() { return map<Properties>(request("GET", "/v1/properties"), Properties::from); }
    /// Merge properties; a null value removes the key.
    Result<Properties> update_properties(Json default_props, Json custom_props = Json::object()) {
        Json body = Json::object();
        body["default"] = default_props.is_object() ? default_props : Json::object();
        body["custom"] = custom_props.is_object() ? custom_props : Json::object();
        return map<Properties>(request("PUT", "/v1/properties", body), Properties::from);
    }
    /// Link the current (anonymous) identity to `id`, merging if it exists; switches to the new session.
    Result<Session> identify(const std::string& id) {
        auto r = request("POST", "/v1/identify", Json{{"id", id}});
        if (!r) return r.error();
        Session s = Session::from(*r);
        set_session(s);
        return s;
    }

    // ---- flags / audiences / experiments -------------------------------------------------------

    Result<std::vector<Flag>> flags(const std::vector<std::string>& names = {}) {
        return list<Flag>(request("GET", "/v1/flags" + names_query(names)), "flags", Flag::from);
    }
    Result<Flag> flag(const std::string& key) { return map<Flag>(request("GET", "/v1/flags/" + url_encode(key)), Flag::from); }
    bool flag_bool(const std::string& key, bool fallback) { auto f = flag(key); return f ? f->value.as_bool(fallback) : fallback; }
    double flag_number(const std::string& key, double fallback) { auto f = flag(key); return f ? f->value.as_number(fallback) : fallback; }
    std::string flag_string(const std::string& key, const std::string& fallback) {
        auto f = flag(key);
        if (!f || f->value.is_null()) return fallback;
        return f->value.is_string() ? f->value.as_string() : f->value.dump();
    }
    Json flag_json(const std::string& key, Json fallback = Json()) { auto f = flag(key); return f ? f->value : fallback; }
    Result<std::vector<std::string>> audiences() {
        auto r = request("GET", "/v1/audiences");
        if (!r) return r.error();
        std::vector<std::string> out;
        for (const auto& a : (*r)["audiences"].as_array()) out.push_back(a.as_string());
        return out;
    }
    Result<std::vector<Experiment>> experiments(const std::vector<std::string>& names = {}) {
        return list<Experiment>(request("GET", "/v1/experiments" + names_query(names)), "experiments", Experiment::from);
    }
    Result<bool> expose(const std::string& experiment) { return ok(request("POST", "/v1/experiments/" + url_encode(experiment) + "/expose")); }

    // ---- live events / inbox -------------------------------------------------------------------

    Result<std::vector<LiveEvent>> live_events(const std::vector<std::string>& names = {}, bool upcoming = false) {
        std::string q = names_query(names);
        if (upcoming) q += (q.empty() ? "?" : "&") + std::string("upcoming=true");
        return list<LiveEvent>(request("GET", "/v1/live-events" + q), "live_events", LiveEvent::from);
    }
    Result<LiveEvent> join_live_event(const std::string& key) { return map<LiveEvent>(request("POST", "/v1/live-events/" + url_encode(key) + "/join"), LiveEvent::from); }
    Result<bool> leave_live_event(const std::string& key) {
        auto r = request("POST", "/v1/live-events/" + url_encode(key) + "/leave");
        if (!r) return r.error();
        return (*r)["left"].as_bool();
    }
    Result<std::vector<InboxMessage>> inbox(int limit = 0) {
        return list<InboxMessage>(request("GET", std::string("/v1/messages") + (limit > 0 ? "?limit=" + std::to_string(limit) : "")), "messages", InboxMessage::from);
    }
    Result<InboxMessage> mark_read(const std::string& id) { return map<InboxMessage>(request("PUT", "/v1/messages/" + url_encode(id), Json{{"read", true}}), InboxMessage::from); }
    Result<bool> consume(const std::string& id) { return ok(request("DELETE", "/v1/messages/" + url_encode(id))); }
    Result<bool> delete_inbox_message(const std::string& id) { return ok(request("DELETE", "/v1/inbox/" + url_encode(id))); }

    // ---- push tokens / analytics ---------------------------------------------------------------

    Result<PushToken> register_push_token(const std::string& platform, const std::string& token, const std::string& device_id = "") {
        return map<PushToken>(request("PUT", "/v1/push-tokens", Json{{"platform", platform}, {"token", token}, {"device_id", device_id}}), PushToken::from);
    }
    Result<std::vector<PushToken>> push_tokens() { return list<PushToken>(request("GET", "/v1/push-tokens"), "tokens", PushToken::from); }
    Result<bool> unregister_push_token(const std::string& token) { return ok(request("DELETE", "/v1/push-tokens/" + url_encode(token))); }

    /// Report events; returns how many were accepted.
    Result<int> track(const std::vector<EventInput>& events) {
        if (events.empty()) return 0;
        Json list = Json::array();
        for (const auto& e : events) {
            Json o = Json::object();
            o["name"] = e.name;
            if (e.value) o["value"] = *e.value;
            if (e.metadata.is_object() && e.metadata.size() > 0) o["metadata"] = e.metadata;
            o["timestamp"] = e.timestamp ? *e.timestamp : now_iso();
            if (e.id) o["id"] = *e.id;
            list.as_array().push_back(o);
        }
        auto r = request("POST", "/v1/events", Json{{"events", list}});
        if (!r) return r.error();
        return static_cast<int>((*r)["accepted"].as_int());
    }
    Result<int> track(const std::string& name, std::optional<std::string> value = std::nullopt, Json metadata = Json()) {
        EventInput e; e.name = name; e.value = std::move(value); e.metadata = std::move(metadata);
        return track(std::vector<EventInput>{e});
    }

    /// Raw call for endpoints not wrapped above (session auth).
    Result<Json> raw(const std::string& method, const std::string& path, const Json& body = Json()) { return request(method, path, body); }

    static std::string url_encode(const std::string& s) {
        static const char* hex = "0123456789ABCDEF";
        std::string out;
        for (unsigned char c : s) {
            if (std::isalnum(c) || c == '-' || c == '_' || c == '.' || c == '~') out += static_cast<char>(c);
            else { out += '%'; out += hex[c >> 4]; out += hex[c & 15]; }
        }
        return out;
    }

private:
    enum class Auth { None, Key, Session };

    ClientOptions opts_;
    std::shared_ptr<HttpTransport> http_;
    mutable std::mutex m_;
    std::mutex refresh_m_;
    std::string token_, refresh_token_;
    long long expires_at_ = 0;

    static std::string now_iso() {
        std::time_t t = std::time(nullptr);
        char buf[32];
        std::strftime(buf, sizeof buf, "%Y-%m-%dT%H:%M:%SZ", std::gmtime(&t));
        return buf;
    }
    static std::string names_query(const std::vector<std::string>& names) {
        if (names.empty()) return "";
        std::string joined;
        for (size_t i = 0; i < names.size(); ++i) { if (i) joined += ','; joined += names[i]; }
        return "?names=" + url_encode(joined);
    }
    static std::string b64_decode(std::string s) {
        for (auto& c : s) { if (c == '-') c = '+'; else if (c == '_') c = '/'; }
        while (s.size() % 4) s += '=';
        static const std::string tbl = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        std::string out;
        unsigned val = 0; int bits = -8;
        for (unsigned char c : s) {
            if (c == '=') break;
            auto p = tbl.find(static_cast<char>(c));
            if (p == std::string::npos) return "";
            val = (val << 6) + static_cast<unsigned>(p); bits += 6;
            if (bits >= 0) { out += static_cast<char>((val >> bits) & 0xFF); bits -= 8; }
        }
        return out;
    }
    static long long jwt_exp(const std::string& jwt) {
        auto a = jwt.find('.');
        if (a == std::string::npos) return 0;
        auto b = jwt.find('.', a + 1);
        std::string payload = jwt.substr(a + 1, b == std::string::npos ? std::string::npos : b - a - 1);
        bool ok = false;
        Json j = Json::try_parse(b64_decode(payload), ok);
        return ok ? j["exp"].as_int() : 0;
    }
    bool expiring_soon() const {
        std::lock_guard<std::mutex> g(m_);
        if (token_.empty()) return true;
        if (expires_at_ == 0) return false;
        return static_cast<long long>(std::time(nullptr)) >= expires_at_ - opts_.refresh_leeway_seconds;
    }
    void set_session(const Session& s) {
        { std::lock_guard<std::mutex> g(m_); token_ = s.token; refresh_token_ = s.refresh_token; expires_at_ = jwt_exp(s.token); }
        if (opts_.on_session) opts_.on_session(s.token, s.refresh_token);
    }
    void clear_session() {
        { std::lock_guard<std::mutex> g(m_); token_.clear(); refresh_token_.clear(); expires_at_ = 0; }
        if (opts_.on_session) opts_.on_session("", "");
    }

    Result<Json> request(const std::string& method, const std::string& path, const Json& body = Json(), Auth auth = Auth::Session) {
        HttpRequest req;
        req.method = method;
        req.url = opts_.url + path;
        req.headers.emplace_back("Accept", "application/json");
        req.headers.emplace_back("User-Agent", opts_.user_agent);
        if (auth == Auth::Key) req.headers.emplace_back("x-api-key", opts_.api_key);
        if (auth == Auth::Session) {
            if (!is_authenticated()) return Error{401, "unauthenticated", "not authenticated"};
            if (expiring_soon() && !refresh_token().empty()) { auto r = refresh(); if (!r) return r.error(); }
            req.headers.emplace_back("Authorization", "Bearer " + token());
        }
        if (!body.is_null()) { req.headers.emplace_back("Content-Type", "application/json"); req.body = body.dump(); }
        HttpResponse res = http_->send(req);
        if (res.status == 0) return Error{0, "transport", res.error.empty() ? "network error" : res.error};
        bool parsed_ok = false;
        Json parsed = res.body.empty() ? Json::object() : Json::try_parse(res.body, parsed_ok);
        if (res.status < 200 || res.status >= 300) {
            Error e{res.status, "http_error", res.body.substr(0, 200)};
            if (parsed_ok && parsed.is_object()) {
                if (parsed.contains("code")) e.code = parsed["code"].as_string();
                if (parsed.contains("message")) e.message = parsed["message"].as_string();
            }
            if (res.status == 401 && auth == Auth::Session) clear_session();
            return e;
        }
        if (!res.body.empty() && !parsed_ok) return Error{res.status, "bad_json", "unparseable response body"};
        return parsed;
    }
    template <typename T>
    static Result<T> map(Result<Json> r, T (*from)(const Json&)) {
        if (!r) return r.error();
        return from(*r);
    }
    template <typename T>
    static Result<std::vector<T>> list(Result<Json> r, const char* field, T (*from)(const Json&)) {
        if (!r) return r.error();
        std::vector<T> out;
        for (const auto& it : (*r)[field].as_array()) out.push_back(from(it));
        return out;
    }
    static Result<bool> ok(Result<Json> r) { if (!r) return r.error(); return true; }
};

}  // namespace zanshin
