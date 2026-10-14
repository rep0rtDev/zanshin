# Zanshin Godot 3 SDK

GDScript addon for Godot 3.5+ (the Godot 4 addon lives in `sdk/godot`). Zero dependencies:
`HTTPRequest` for calls, `WebSocketClient` for the live stream. Every call is a coroutine returning
`ZanshinTypes.Result` (`ok`, `value`, `err`).

## Install

Copy `addons/zanshin` into your project's `addons/` folder and enable **Zanshin** in
*Project → Project Settings → Plugins*. The plugin registers a `Zanshin` autoload.

## Use

```gdscript
func _ready() -> void:
	Zanshin.configure("https://liveops.example.com", "zk_c_...")   # client API key
	Zanshin.connect("session_changed", self, "_persist_tokens")    # save both tokens; feed to restore()
	var r = yield(Zanshin.authenticate(player_id, {"platform": OS.get_name().to_lower()}, {"level": 3}), "completed")
	if not r.ok:
		push_warning(str(r.err)); return

	if yield(Zanshin.flag_bool("new_shop"), "completed"):          # typed helpers with fallbacks
		show_new_shop()
	var coins = yield(Zanshin.flag_number("daily_reward_coins", 100.0), "completed")

	var exps = yield(Zanshin.experiments(), "completed")           # Result → Array of ZanshinTypes.Experiment
	yield(Zanshin.expose("onboarding_v2"), "completed")
	yield(Zanshin.identify("account-42"), "completed")             # anonymous → account, merges if it exists
	yield(Zanshin.join_live_event("weekend_boost"), "completed")
	yield(Zanshin.track("level_completed", 3, {"stars": 2}), "completed")

	var s = Zanshin.stream(PoolStringArray(["new_shop"]))         # live stream (child Node)
	s.connect("flag_changed", self, "_on_flag")
	s.connect_stream()
```

Same surface as the Godot 4 addon: sessions (`authenticate` / `refresh` / `restore` / `logout`),
identity & properties, flags (`flags`, `flag`, `flag_bool/string/number`), audiences, experiments +
`expose`, `identify`, live events + `join/leave`, inbox (`inbox`, `mark_read`, `consume`,
`delete_inbox_message`), push tokens, `track` / `track_events`, and `stream()` with signals
`connected`, `disconnected`, `error`, `flag_changed`, `flag_deleted`, `flags_changed`,
`experiments_changed`, `live_events_changed`, `inbox_changed`.

Differences from Godot 4: results come back through `yield(..., "completed")`; `ZanshinTypes.map`
takes the type name instead of a lambda; the stream refreshes the session on any unexpected close
(Godot 3 does not expose WebSocket close codes).

## Smoke test

`godot3 --no-window --path smoke --script res://smoke.gd` parses the addon and, when
`ZANSHIN_URL` / `ZANSHIN_CLIENT_KEY` are set, runs the live flow against a server.
