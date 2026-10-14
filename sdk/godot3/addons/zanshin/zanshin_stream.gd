extends Node
# Live change stream over /v1/stream using WebSocketClient (Godot 3.x): reconnects with jittered
# backoff, pings every 30 s, keeps `flags` current from pushes and refreshes the session when it expires.

const T = preload("zanshin_types.gd")

signal connected
signal disconnected(reason)
signal error(err)                       # ZanshinTypes.ZError
signal flag_changed(flag)               # ZanshinTypes.Flag
signal flag_deleted(key)
signal flags_changed(flags)
signal experiments_changed(experiments)
signal live_events_changed(events)
signal inbox_changed(messages)

var flags: Dictionary = {}   # key -> ZanshinTypes.Flag

var _client: Node
var _names: PoolStringArray
var _ws: WebSocketClient = null
var _open := false
var _closed := true
var _attempt := 0
var _ping_left := 30.0
var _reconnect_left := -1.0

func setup(client: Node, flag_names: PoolStringArray) -> void:
	_client = client
	_names = flag_names

func connect_stream() -> void:
	_closed = false
	_attempt = 0
	_open_socket()

func close() -> void:
	_closed = true
	_reconnect_left = -1.0
	if _ws != null and _open:
		_ws.disconnect_from_host()
	_open = false

func is_connected_to_server() -> bool:
	return _open

func _open_socket() -> void:
	if _closed: return
	if not _client.is_authenticated():
		emit_signal("error", T.ZError.new(401, "unauthenticated", "stream needs a session"))
		_schedule_reconnect()
		return
	var base: String = _client.url
	if base.begins_with("https://"): base = "wss://" + base.substr(8)
	elif base.begins_with("http://"): base = "ws://" + base.substr(7)
	var q = "token=" + (_client.token as String).percent_encode()
	if _names.size() > 0: q += "&names=" + _names.join(",").percent_encode()
	_ws = WebSocketClient.new()
	_ws.connect("connection_established", self, "_on_established")
	_ws.connect("connection_closed", self, "_on_closed")
	_ws.connect("connection_error", self, "_on_error")
	_ws.connect("data_received", self, "_on_data")
	var err = _ws.connect_to_url(base + "/v1/stream?" + q)
	if err != OK:
		emit_signal("error", T.ZError.new(0, "transport", "connect error %d" % err))
		_ws = null
		_schedule_reconnect()

func _process(delta: float) -> void:
	if _reconnect_left >= 0.0:
		_reconnect_left -= delta
		if _reconnect_left < 0.0:
			_reconnect_left = -1.0
			_open_socket()
		return
	if _ws == null: return
	_ws.poll()
	if _open:
		_ping_left -= delta
		if _ping_left <= 0.0:
			_ping_left = 30.0
			_ws.get_peer(1).put_packet('{"type":"ping"}'.to_utf8())

func _on_established(_protocol: String) -> void:
	_open = true; _attempt = 0; _ping_left = 30.0
	_ws.get_peer(1).set_write_mode(WebSocketPeer.WRITE_MODE_TEXT)
	emit_signal("connected")

func _on_data() -> void:
	while _ws != null and _ws.get_peer(1).get_available_packet_count() > 0:
		_handle(_ws.get_peer(1).get_packet().get_string_from_utf8())

func _on_error() -> void:
	var was_open = _open
	_open = false
	_ws = null
	if was_open: emit_signal("disconnected", "error")
	else: emit_signal("error", T.ZError.new(0, "transport", "connection error"))
	if not _closed: _schedule_reconnect()

func _on_closed(_was_clean: bool) -> void:
	var was_open = _open
	_open = false
	_ws = null
	if was_open: emit_signal("disconnected", "closed")
	if _closed: return
	# Godot 3 does not expose the close code: refresh the session before reconnecting when the
	# socket closes while open, which covers token expiry (4401) at the cost of one extra call.
	if was_open: _refresh_then_reconnect()
	else: _schedule_reconnect()

func _refresh_then_reconnect() -> void:
	yield(_client.refresh(), "completed")
	_schedule_reconnect()

func _schedule_reconnect() -> void:
	if _closed or _reconnect_left >= 0.0: return
	var base: float = min(30.0, 0.5 * pow(2.0, float(min(_attempt, 6))))
	_reconnect_left = base + randf() * base * 0.3
	_attempt += 1

func _handle(text: String) -> void:
	var p = JSON.parse(text)
	if p.error != OK or not (p.result is Dictionary): return
	var m: Dictionary = p.result
	match str(m.get("type", "")):
		"flag":
			if m.get("flag") is Dictionary:
				var f = T.Flag.new(m["flag"])
				flags[f.key] = f
				emit_signal("flag_changed", f)
		"flag_deleted":
			var k = str(m.get("key", ""))
			flags.erase(k)
			emit_signal("flag_deleted", k)
		"flags":
			var all = T.map(m.get("flags"), "Flag")
			for f in all: flags[f.key] = f
			emit_signal("flags_changed", all)
		"experiments": emit_signal("experiments_changed", T.map(m.get("experiments"), "Experiment"))
		"live_events": emit_signal("live_events_changed", T.map(m.get("live_events"), "LiveEvent"))
		"inbox": emit_signal("inbox_changed", T.map(m.get("messages"), "InboxMessage"))
		"error":
			var e = T.ZError.new(0, str(m.get("code", "error")), str(m.get("message", "")))
			emit_signal("error", e)
			if e.message.to_lower().find("expired") >= 0:
				_client.refresh()
