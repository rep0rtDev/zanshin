# Zanshin C++17 SDK

Header-only, dependency-free client for Cocos2d-x, SDL, custom engines or any C++17 code base.
Bring your own HTTP transport (a libcurl adapter is included); all calls are synchronous and
return `zanshin::Result<T>` — run them on a worker thread or wrap them in your engine's job system.

## Install

Add `include/` to your include path (or `add_subdirectory(sdk/cpp)` and link `zanshin` with CMake).

## Use

```cpp
#include <zanshin/zanshin.hpp>
#include <zanshin/curl_transport.hpp>   // -lcurl; or implement zanshin::HttpTransport yourself

zanshin::ClientOptions opts;
opts.url = "https://liveops.example.com";
opts.api_key = "zk_c_...";                                   // client API key
opts.on_session = [](const std::string& token, const std::string& refresh) { save(token, refresh); };
zanshin::Client z(opts, std::make_shared<zanshin::CurlTransport>());

auto s = z.authenticate(player_id, {{"platform", "ios"}}, {{"level", 3}});
if (!s) { log(s.error().to_string()); return; }

if (z.flag_bool("new_shop", false)) show_new_shop();           // typed helpers with fallbacks
double coins = z.flag_number("daily_reward_coins", 100);
auto exps = z.experiments();                                  // Result<std::vector<Experiment>>
z.expose("onboarding_v2");
z.identify("account-42");                                     // anonymous → account, merges if it exists
z.join_live_event("weekend_boost");
z.track("level_completed", std::string("3"), {{"stars", 2}});
for (auto& m : z.inbox().value_or({})) z.mark_read(m.id);
```

Tokens refresh automatically before they expire (`refresh_leeway_seconds`); call
`z.restore(token, refresh_token)` on startup to resume a saved session. `z.raw(method, path, body)`
reaches any endpoint not wrapped here.

Surface: `authenticate`, `refresh`, `restore`, `logout`, `identity`, `delete_identity`,
`properties`, `update_properties`, `identify`, `flags`, `flag`, `flag_bool/number/string/json`,
`audiences`, `experiments`, `expose`, `live_events`, `join_live_event`, `leave_live_event`,
`inbox`, `mark_read`, `consume`, `delete_inbox_message`, `register_push_token`, `push_tokens`,
`unregister_push_token`, `track`.

The live WebSocket stream (`/v1/stream`) is not wrapped: connect with your engine's WebSocket
client using `?token=<access token>` and parse the JSON frames described in `docs/api.md`.

## Transport

```cpp
struct MyTransport : zanshin::HttpTransport {
  zanshin::HttpResponse send(const zanshin::HttpRequest& req) override { /* method, url, headers, body → status, body */ }
};
```

## Test

`make test` (g++ or clang++) or `cmake -B build && cmake --build build && ctest --test-dir build`.
