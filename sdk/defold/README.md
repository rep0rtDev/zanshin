# Zanshin Defold SDK

Pure-Lua client for Defold (Lua 5.1): sessions, feature flags, experiments, live events, inbox,
push tokens, analytics and (with [extension-websocket](https://github.com/defold/extension-websocket))
the live change stream. Only Defold's built-in `http` and `json` are used.

## Install

Add this to the `dependencies` of your `game.project`:

```
https://github.com/rep0rtDev/zanshin/archive/refs/heads/main.zip
```

(Defold picks up the `sdk/defold/zanshin` library folder — set `library.include_dirs = zanshin` if
you vendor it.) Then `local zanshin = require "zanshin.zanshin"`.

## Use

```lua
local zanshin = require "zanshin.zanshin"

function init(self)
	self.z = zanshin.new({
		url = "https://liveops.example.com",
		api_key = "zk_c_...",                              -- client API key
		on_session = function(token, refresh) save_tokens(token, refresh) end,  -- nil = logged out
	})
	self.z:authenticate(player_id, { default = { platform = sys.get_sys_info().system_name }, custom = { level = 3 } }, function(ok, s)
		if not ok then print("auth failed", s.status, s.message) return end
		self.z:flag_bool("new_shop", false, function(on) if on then show_new_shop() end end)
		self.z:flag_number("daily_reward_coins", 100, function(coins) self.coins = coins end)
		self.z:experiments(function(ok, exps) ... end)
		self.z:expose("onboarding_v2")
		self.z:track("level_completed", 3, { stars = 2 })
		self.stream = self.z:stream({ on_flag = function(f) apply_flag(f) end })
	end)
end

function final(self) if self.stream then self.stream.close() end end
```

Every call takes a trailing `cb(ok, value_or_error)`; errors are `{ status, code, message }`.
Tokens refresh automatically before they expire (`refresh_leeway_seconds`, default 30); call
`z:restore(token, refresh_token)` on startup to resume a saved session.

Surface: `authenticate`, `refresh`, `restore`, `logout`, `identity`, `delete_identity`,
`properties`, `update_properties`, `identify`, `flags`, `flag`, `flag_bool/number/string`,
`audiences`, `experiments`, `expose`, `live_events`, `join_live_event`, `leave_live_event`,
`inbox`, `mark_read`, `consume`, `delete_inbox_message`, `register_push_token`,
`unregister_push_token`, `track`, `track_events`, `stream`.

## Test

`python3 test/run.py` (uses [lupa](https://pypi.org/project/lupa/) for a Lua 5.1 runtime and a fake
`http`), or any Lua 5.1 with `lua test/test.lua` and a JSON module exposed as `json`.
