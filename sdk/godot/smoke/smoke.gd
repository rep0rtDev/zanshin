extends SceneTree
## Runs the SDK against a live server. Exits 0 on success, 1 on failure.

var _fail := 0

func check(cond: bool, label: String) -> void:
	print(("ok   " if cond else "FAIL ") + label)
	if not cond: _fail += 1

func _init() -> void:
	call_deferred("_run")

func _run() -> void:
	var url := OS.get_environment("ZANSHIN_TEST_URL")
	var key := OS.get_environment("ZANSHIN_TEST_CLIENT_KEY")
	if url == "" or key == "":
		print("skipping: ZANSHIN_TEST_URL / ZANSHIN_TEST_CLIENT_KEY not set")
		quit(0); return
	var z: Node = load("res://addons/zanshin/zanshin.gd").new()
	root.add_child(z)
	z.configure(url, key)
	var changes := []
	z.session_changed.connect(func(t, _r): changes.append(t))

	var pid := "godot-%d" % randi()
	var r = await z.authenticate(pid, {"platform": "godot", "country": "DE"}, {"level": 2})
	check(r.ok and z.is_authenticated(), "authenticate")
	check(changes.size() == 1, "session_changed emitted")

	r = await z.get_properties()
	check(r.ok and r.value.get("default", {}).get("platform") == "godot", "properties merge")

	r = await z.flags()
	check(r.ok and r.value is Array, "flags list (%d)" % (r.value.size() if r.ok else -1))
	r = await z.flag("does_not_exist_%d" % randi())
	check(not r.ok and r.err.is_not_found(), "missing flag 404")
	check((await z.flag_bool("does_not_exist", true)) == true, "flag_bool fallback")
	check((await z.flag_number("does_not_exist", 7.0)) == 7.0, "flag_number fallback")

	r = await z.audiences(); check(r.ok and r.value is Array, "audiences")
	r = await z.experiments(); check(r.ok, "experiments")
	r = await z.live_events(); check(r.ok, "live events")
	r = await z.inbox(); check(r.ok, "inbox")

	r = await z.track_events([{"name": "godot.ping"}, {"name": "godot.level_up", "value": 3, "metadata": {"map": "a"}}])
	check(r.ok and r.value == 2, "track 2 -> accepted %s" % str(r.value if r.ok else r.err))
	r = await z.track("$reserved")
	check(not r.ok and r.err.status == 400, "reserved event 400")

	r = await z.register_push_token("fcm", "godot-token-%d" % randi(), "dev-1")
	check(r.ok, "push token register")

	var old: String = z.token
	r = await z.refresh()
	check(r.ok and z.token != old, "refresh rotates")

	var s: Node = z.stream()
	var got := {"connected": false, "flags": false}
	s.connected.connect(func(): got["connected"] = true)
	s.flags_changed.connect(func(_f): got["flags"] = true)
	s.connect_stream()
	var waited := 0.0
	while not got["connected"] and waited < 5.0:
		await create_timer(0.1).timeout
		waited += 0.1
	check(got["connected"], "stream connected")
	s.close()

	r = await z.logout()
	check(r.ok and not z.is_authenticated(), "logout")
	r = await z.flags()
	check(not r.ok and r.err.is_unauthenticated(), "unauthenticated after logout")
	print("ALL OK" if _fail == 0 else "%d FAILED" % _fail)
	quit(1 if _fail > 0 else 0)
