-- Unit test for the Defold SDK with a fake `http` transport (run through test/run.py or lua5.1).
local zanshin = require "zanshin.zanshin"

local calls = {}
local responses = {}
local function fake_http(url, method, cb, headers, body)
	calls[#calls + 1] = { url = url, method = method, headers = headers, body = body and json.decode(body) or nil }
	local r = table.remove(responses, 1) or { status = 200, response = "{}" }
	cb(r)
end

local sessions = {}
local z = zanshin.new({ url = "https://z.example/", api_key = "zk_c_test", http = fake_http,
	on_session = function(t, rt) sessions[#sessions + 1] = { t, rt } end })

-- unauthenticated session call fails without touching the network
local got
z:flags(function(ok, e) got = { ok, e } end)
assert(got[1] == false and got[2].status == 401, "needs session")
assert(#calls == 0)

-- authenticate uses the API key and stores tokens
local header = "eyJhbGciOiJIUzI1NiJ9"   -- {"alg":"HS256"}
local far = "eyJleHAiOjQxMDI0NDQ4MDB9"  -- {"exp":4102444800}
local tok = header .. "." .. far .. ".sig"
responses[1] = { status = 200, response = json.encode({ token = tok, refresh_token = "r1", identity = { id = "i1", external_id = "p1" } }) }
z:authenticate("p1", { custom = { level = 3 } }, function(ok, s) got = { ok, s } end)
assert(got[1] == true and got[2].identity.external_id == "p1")
assert(calls[1].method == "POST" and calls[1].url == "https://z.example/v1/authenticate")
assert(calls[1].headers["x-api-key"] == "zk_c_test" and calls[1].body.custom.level == 3)
assert(z:is_authenticated() and sessions[1][1] == tok)

-- flags: bearer auth, typed helpers
responses[1] = { status = 200, response = json.encode({ flags = { { key = "a", value = "true", value_type = "bool" } } }) }
z:flags({ "a" }, function(ok, flags) got = { ok, flags } end)
assert(got[1] and #got[2] == 1 and got[2][1].key == "a")
assert(calls[#calls].headers["Authorization"] == "Bearer " .. tok)
assert(calls[#calls].url == "https://z.example/v1/flags?names=a")
responses[1] = { status = 200, response = json.encode({ key = "n", value = "2.5", value_type = "number" }) }
local num
z:flag_number("n", 1, function(v) num = v end)
assert(num == 2.5)
responses[1] = { status = 404, response = json.encode({ code = "not_found", message = "no flag" }) }
local b
z:flag_bool("missing", true, function(v) b = v end)
assert(b == true, "fallback on 404")
assert(z:is_authenticated(), "404 must not drop the session")

-- track: value stringified, timestamp attached
responses[1] = { status = 200, response = json.encode({ accepted = 1, rejected = 0 }) }
z:track("level_completed", 3, { stars = 2 }, function(ok, n) got = { ok, n } end)
assert(got[1] and got[2] == 1)
local ev = calls[#calls].body.events[1]
assert(ev.name == "level_completed" and ev.value == "3" and ev.metadata.stars == 2 and ev.timestamp:match("Z$"))

-- expired token triggers a refresh before the call
local expired = header .. ".eyJleHAiOjF9.sig"  -- {"exp":1}
z:restore(expired, "r1")
responses[1] = { status = 200, response = json.encode({ token = tok, refresh_token = "r2", identity = { id = "i1" } }) }
responses[2] = { status = 200, response = json.encode({ audiences = { "whales" } }) }
z:audiences(function(ok, a) got = { ok, a } end)
assert(got[1] and got[2][1] == "whales")
assert(calls[#calls - 1].url:match("/v1/authenticate/refresh$") and calls[#calls - 1].body.refresh_token == "r1")
assert(z.refresh_token == "r2")

-- 401 on a session call clears the session
responses[1] = { status = 401, response = json.encode({ code = "unauthenticated", message = "revoked" }) }
z:inbox(function(ok, e) got = { ok, e } end)
assert(got[1] == false and got[2].code == "unauthenticated")
assert(not z:is_authenticated() and sessions[#sessions][1] == nil)

-- logout without a session is a no-op
z:logout(function(ok) got = { ok } end)
assert(got[1] == true)

print("defold sdk tests OK (" .. #calls .. " fake requests)")
