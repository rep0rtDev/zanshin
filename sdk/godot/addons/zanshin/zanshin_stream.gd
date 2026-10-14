extends Node
## Live change stream over /v1/stream using WebSocketPeer: reconnects with jittered backoff, pings
## every 30 s, keeps `flags` current from pushes and refreshes the session when it expires.

const T := preload("zanshin_types.gd")

signal connected
signal disconnected(reason: String)
signal error(err)                       ## ZanshinTypes.ZError
signal flag_changed(flag)               ## ZanshinTypes.Flag
signal flag_deleted(key: String)
signal flags_changed(flags: Array)
signal experiments_changed(experiments: Array)
signal live_events_changed(events: Array)
signal inbox_changed(messages: Array)

var flags: Dictionary = {}   ## key -> ZanshinTypes.Flag

var _client: Node
var _names: PackedStringArray
var _ws: WebSocketPeer
var _open := false
var _closed := true
var _attempt := 0
var _ping_left := 30.0
var _reconnect_left := -1.0

func setup(client: Node, flag_names: PackedStringArray) -> void:
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
		_ws.close()
	_open = false

func is_connected_to_server() -> bool:
	return _open

func _open_socket() -> void:
	if _closed: return
	if not _client.is_authenticated():
		error.emit(T.ZError.new(401, "unauthenticated", "stream needs a session"))
		_schedule_reconnect()
		return
	var base: String = _client.url
	if base.begins_with("https://"): base = "wss://" + base.substr(8)
	elif base.begins_with("http://"): base = "ws://" + base.substr(7)
	var q := "token=" + (_client.token as String).uri_encode()
	if _names.size() > 0: q += "&names=" + ",".join(_names).uri_encode()
	_ws = WebSocketPeer.new()
	var err := _ws.connect_to_url(base + "/v1/stream?" + q)
	if err != OK:
		error.emit(T.ZError.new(0, "transport", "connect error %d" % err))
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
	match _ws.get_ready_state():
		WebSocketPeer.STATE_OPEN:
			if not _open:
				_open = true; _attempt = 0; _ping_left = 30.0
				connected.emit()
			_ping_left -= delta
			if _ping_left <= 0.0:
				_ping_left = 30.0
				_ws.send_text('{"type":"ping"}')
			while _ws.get_available_packet_count() > 0:
				_handle(_ws.get_packet().get_string_from_utf8())
		WebSocketPeer.STATE_CLOSED:
			var code := _ws.get_close_code()
			var reason := _ws.get_close_reason()
			_ws = null
			var was_open := _open
			_open = false
			if was_open: disconnected.emit("%d %s" % [code, reason])
			if _closed: return
			if code == 4401 or code == 1008:
				_refresh_then_reconnect()
			else:
				_schedule_reconnect()

func _refresh_then_reconnect() -> void:
	await _client.refresh()
	_schedule_reconnect()

func _schedule_reconnect() -> void:
	if _closed or _reconnect_left >= 0.0: return
	var base: float = min(30.0, 0.5 * pow(2.0, float(min(_attempt, 6))))
	_reconnect_left = base + randf() * base * 0.3
	_attempt += 1

func _handle(text: String) -> void:
	var m: Variant = JSON.parse_string(text)
	if not (m is Dictionary): return
	match str(m.get("type", "")):
		"flag":
			if m.get("flag") is Dictionary:
				var f := T.Flag.new(m["flag"])
				flags[f.key] = f
				flag_changed.emit(f)
		"flag_deleted":
			var k := str(m.get("key", ""))
			flags.erase(k)
			flag_deleted.emit(k)
		"flags":
			var all := T.map(m.get("flags"), func(d): return T.Flag.new(d))
			for f in all: flags[f.key] = f
			flags_changed.emit(all)
		"experiments": experiments_changed.emit(T.map(m.get("experiments"), func(d): return T.Experiment.new(d)))
		"live_events": live_events_changed.emit(T.map(m.get("live_events"), func(d): return T.LiveEvent.new(d)))
		"inbox": inbox_changed.emit(T.map(m.get("messages"), func(d): return T.InboxMessage.new(d)))
		"error":
			var e := T.ZError.new(0, str(m.get("code", "error")), str(m.get("message", "")))
			error.emit(e)
			if e.message.to_lower().contains("expired"):
				_client.refresh()
