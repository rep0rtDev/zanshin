# Zanshin Godot SDK

GDScript addon for Godot 4.3+. Zero dependencies: `HTTPRequest` for calls, `WebSocketPeer` for the
live stream. Every call is awaitable and returns `ZanshinTypes.Result` (`ok`, `value`, `err`).

## Install

Copy `addons/zanshin` into your project's `addons/` folder and enable **Zanshin** in
*Project → Project Settings → Plugins*. The plugin registers a `Zanshin` autoload.

## Use

```gdscript
func _ready() -> void:
	Zanshin.configure("https://liveops.example.com", "zk_c_...")   # client API key
	Zanshin.session_changed.connect(_persist_tokens)              # save both tokens; feed to restore()
	var r := await Zanshin.authenticate(player_id, {"platform": OS.get_name().to_lower()}, {"level": 3})
	if not r.ok:
		push_warning(str(r.err)); return

	if await Zanshin.flag_bool("new_shop"):                          # typed helpers with fallbacks
		show_new_shop()
	var coins := await Zanshin.flag_number("daily_reward_coins", 100.0)

	var exps := await Zanshin.experiments()                         # Array[ZanshinTypes.Experiment]
	await Zanshin.expose("onboarding_v2")
	var inbox := await Zanshin.inbox(20)                            # Array[ZanshinTypes.InboxMessage]
	await Zanshin.track("level.up", 4, {"map": "forest"})

	var s := Zanshin.stream(["new_shop", "daily_reward_coins"])     # live updates
	s.flag_changed.connect(func(f): print(f.key, " -> ", f.value))
	s.inbox_changed.connect(func(msgs): refresh_inbox(msgs))
	s.connect_stream()
```

- Session calls refresh the access token automatically shortly before it expires (expiry is read
  from the JWT); concurrent refreshes are coalesced. A `401` clears tokens and emits
  `session_changed("", "")`.
- `Zanshin.restore(token, refresh_token)` on startup to skip re-authentication.
- `stream()` returns a child `Node`; `flags` on it is a `Dictionary` kept current from pushes.
  Reconnects with jittered backoff (≤30 s), pings every 30 s, refreshes the session on auth close.
- Other calls: `get_identity`, `delete_identity`, `get_properties`, `update_properties`, `flags`,
  `flag`, `audiences`, `live_events(names, upcoming)`, `mark_read`, `consume`,
  `register_push_token("fcm"|"apns", token, device_id)`, `unregister_push_token`, `track_events`,
  `logout`.

## Smoke test

`smoke/` is a headless project that runs the full flow against a live server:

```sh
ZANSHIN_TEST_URL=http://localhost:7550 ZANSHIN_TEST_CLIENT_KEY=zk_c_... \
  godot --headless --path sdk/godot/smoke --script res://smoke.gd
```
