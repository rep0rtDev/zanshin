extends SceneTree
# Parses the addon (compile errors fail the run) and exercises the live flow when a server is given.

func _init() -> void:
	call_deferred("_run")

func _run() -> void:
	var client = load("res://addons/zanshin/zanshin.gd").new()
	root.add_child(client)
	var T = load("res://addons/zanshin/zanshin_types.gd")
	var f = T.Flag.new({"key": "k", "value": "true", "value_type": "bool", "reason": {"kind": "default"}, "version": 1})
	assert(f.as_bool() == true)
	assert(T.Flag.new({"value": "2.5"}).as_number() == 2.5)
	assert(T.map([{"key": "a"}, {"key": "b"}], "Experiment").size() == 2)
	var url = OS.get_environment("ZANSHIN_URL")
	var key = OS.get_environment("ZANSHIN_CLIENT_KEY")
	if url == "" or key == "":
		print("godot3 addon parsed OK (no server configured, live flow skipped)")
		quit(0)
		return
	client.configure(url, key)
	var r = yield(client.authenticate("smoke-godot3-%d" % OS.get_unix_time(), {"platform": "linux"}), "completed")
	if not r.ok:
		printerr("authenticate failed: ", str(r.err)); quit(1); return
	var fl = yield(client.flags(), "completed")
	if not fl.ok:
		printerr("flags failed: ", str(fl.err)); quit(1); return
	var tr = yield(client.track("smoke", 1, {"from": "godot3"}), "completed")
	if not tr.ok or tr.value != 1:
		printerr("track failed"); quit(1); return
	yield(client.logout(), "completed")
	print("godot3 live flow OK (%d flags)" % fl.value.size())
	quit(0)
