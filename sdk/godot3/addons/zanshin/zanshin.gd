extends Node
# Zanshin client for one identity (Godot 3.x). Added as the `Zanshin` autoload by the plugin.
# Every call is a coroutine: `var r = yield(Zanshin.flags(), "completed")` returns a
# `ZanshinTypes.Result` (`ok`, `value`, `err`); the *_or helpers return plain values.
#
#   Zanshin.configure("https://liveops.example.com", "zk_c_...")
#   var r = yield(Zanshin.authenticate(player_id, {"platform": OS.get_name().to_lower()}), "completed")
#   if yield(Zanshin.flag_bool("new_shop"), "completed"): ...

const T = preload("zanshin_types.gd")
const StreamScript = preload("zanshin_stream.gd")

signal session_changed(token, refresh_token)   # empty strings = logged out / revoked

var url: String = ""
var api_key: String = ""
var token: String = ""
var refresh_token: String = ""
var refresh_leeway_seconds: int = 30
var user_agent: String = "zanshin-godot3/0.1"

var _expires_at: int = 0
var _refreshing: bool = false
var _refresh_result = null

enum Auth { NONE, API_KEY, SESSION }

func configure(base_url: String, client_api_key: String) -> void:
	url = base_url.rstrip("/")
	api_key = client_api_key

func is_authenticated() -> bool:
	return token != ""

# Re-use persisted tokens; refreshes lazily when the access token is stale.
func restore(access_token: String, p_refresh_token: String) -> void:
	token = access_token
	refresh_token = p_refresh_token
	_expires_at = _jwt_exp(token)

# ---- session ------------------------------------------------------------------------------------

func authenticate(id: String, default_props: Dictionary = {}, custom_props: Dictionary = {}):
	var body = {"id": id}
	if not default_props.empty(): body["default"] = default_props
	if not custom_props.empty(): body["custom"] = custom_props
	var r = yield(_request("POST", "/v1/authenticate", body, Auth.API_KEY), "completed")
	if r.ok:
		var s = T.Session.new(r.value)
		_apply_session(s)
		return T.Result.success(s)
	return r

func refresh():
	if _refreshing:
		while _refreshing: yield(get_tree(), "idle_frame")
		return _refresh_result
	if refresh_token == "":
		yield(get_tree(), "idle_frame")
		return T.Result.failure(T.ZError.new(401, "unauthenticated", "no refresh token"))
	_refreshing = true
	var r = yield(_request("POST", "/v1/authenticate/refresh", {"refresh_token": refresh_token}, Auth.API_KEY), "completed")
	if r.ok:
		var s = T.Session.new(r.value)
		_apply_session(s)
		_refresh_result = T.Result.success(s)
	else:
		if r.err.status == 401: _clear_session()
		_refresh_result = r
	_refreshing = false
	return _refresh_result

func logout():
	if token == "":
		yield(get_tree(), "idle_frame")
		return T.Result.success(null)
	var body = {"token": token, "refresh_token": refresh_token}
	_clear_session()
	return yield(_request("POST", "/v1/authenticate/logout", body, Auth.API_KEY), "completed")

# ---- identity & properties ----------------------------------------------------------------------

func get_identity(): return yield(_request("GET", "/v1/identity"), "completed")
func delete_identity():
	var r = yield(_request("DELETE", "/v1/identity"), "completed")
	if r.ok: _clear_session()
	return r
func get_properties(): return yield(_request("GET", "/v1/properties"), "completed")
func update_properties(default_props: Dictionary = {}, custom_props: Dictionary = {}):
	var body = {}
	if not default_props.empty(): body["default"] = default_props
	if not custom_props.empty(): body["custom"] = custom_props
	return yield(_request("PUT", "/v1/properties", body), "completed")

# ---- flags --------------------------------------------------------------------------------------

func flags(names: PoolStringArray = PoolStringArray()):
	var r = yield(_request("GET", "/v1/flags" + _names_q(names)), "completed")
	return T.Result.success(T.map(r.value.get("flags"), "Flag")) if r.ok else r

func flag(key: String):
	var r = yield(_request("GET", "/v1/flags/" + key.percent_encode()), "completed")
	return T.Result.success(T.Flag.new(r.value)) if r.ok else r

func flag_bool(key: String, fallback: bool = false):
	var r = yield(flag(key), "completed")
	return r.value.as_bool(fallback) if r.ok else fallback
func flag_string(key: String, fallback: String = ""):
	var r = yield(flag(key), "completed")
	return r.value.as_string(fallback) if r.ok else fallback
func flag_number(key: String, fallback: float = 0.0):
	var r = yield(flag(key), "completed")
	return r.value.as_number(fallback) if r.ok else fallback

func audiences():
	var r = yield(_request("GET", "/v1/audiences"), "completed")
	return T.Result.success(Array(r.value.get("audiences", []))) if r.ok else r

# ---- experiments --------------------------------------------------------------------------------

func experiments(names: PoolStringArray = PoolStringArray()):
	var r = yield(_request("GET", "/v1/experiments" + _names_q(names)), "completed")
	return T.Result.success(T.map(r.value.get("experiments"), "Experiment")) if r.ok else r

func expose(experiment: String):
	return yield(_request("POST", "/v1/experiments/" + experiment.percent_encode() + "/expose"), "completed")

# ---- live events / inbox ------------------------------------------------------------------------

func live_events(names: PoolStringArray = PoolStringArray(), upcoming: bool = false):
	var q = _names_q(names)
	if upcoming: q += ("&" if q != "" else "?") + "upcoming=true"
	var r = yield(_request("GET", "/v1/live-events" + q), "completed")
	return T.Result.success(T.map(r.value.get("live_events"), "LiveEvent")) if r.ok else r

func inbox(limit: int = 0):
	var r = yield(_request("GET", "/v1/messages" + ("?limit=%d" % limit if limit > 0 else "")), "completed")
	return T.Result.success(T.map(r.value.get("messages"), "InboxMessage")) if r.ok else r

func mark_read(id: String):
	var r = yield(_request("PUT", "/v1/messages/" + id.percent_encode(), {"read": true}), "completed")
	return T.Result.success(T.InboxMessage.new(r.value)) if r.ok else r

func consume(id: String):
	return yield(_request("DELETE", "/v1/messages/" + id.percent_encode()), "completed")

# Remove a message from the inbox entirely (consume keeps it from being redelivered).
func delete_inbox_message(id: String):
	return yield(_request("DELETE", "/v1/inbox/" + id.percent_encode()), "completed")

# Explicitly join a live event with join_mode "explicit"; returns the event as now seen.
func join_live_event(key: String):
	var r = yield(_request("POST", "/v1/live-events/" + key.percent_encode() + "/join"), "completed")
	return T.Result.success(T.LiveEvent.new(r.value)) if r.ok else r

# Leave a live event; the result value is false when the player was not a participant.
func leave_live_event(key: String):
	var r = yield(_request("POST", "/v1/live-events/" + key.percent_encode() + "/leave"), "completed")
	return T.Result.success(bool(r.value.get("left", false))) if r.ok else r

# Link the current (typically anonymous) identity to `id` (merging if it exists); switches to the new session.
func identify(id: String):
	var r = yield(_request("POST", "/v1/identify", {"id": id}), "completed")
	if r.ok:
		var s = T.Session.new(r.value)
		_apply_session(s)
		return T.Result.success(s)
	return r

# ---- push / analytics ---------------------------------------------------------------------------

func register_push_token(platform: String, push_token: String, device_id: String = ""):
	var body = {"platform": platform, "token": push_token}
	if device_id != "": body["device_id"] = device_id
	return yield(_request("PUT", "/v1/push-tokens", body), "completed")

func unregister_push_token(push_token: String):
	return yield(_request("DELETE", "/v1/push-tokens/" + push_token.percent_encode()), "completed")

# Report events: each item is {"name": String, "value" (optional), "metadata": Dictionary (optional)}.
# Returns the number of accepted events.
func track_events(events: Array):
	var list: Array = []
	for e in events:
		var o = {"name": e.get("name", ""), "timestamp": _now_iso()}
		if e.has("value") and e["value"] != null:
			o["value"] = e["value"] if e["value"] is String else JSON.print(e["value"])
		if e.get("metadata") is Dictionary and not e["metadata"].empty(): o["metadata"] = e["metadata"]
		list.append(o)
	var r = yield(_request("POST", "/v1/events", {"events": list}), "completed")
	return T.Result.success(int(r.value.get("accepted", 0))) if r.ok else r

func track(name: String, value = null, metadata: Dictionary = {}):
	return yield(track_events([{"name": name, "value": value, "metadata": metadata}]), "completed")

# Live WebSocket stream (a child Node you can connect signals to).
func stream(flag_names: PoolStringArray = PoolStringArray()) -> Node:
	var s: Node = StreamScript.new()
	s.setup(self, flag_names)
	add_child(s)
	return s

# ---- transport ----------------------------------------------------------------------------------

func _request(method: String, path: String, body = null, auth: int = Auth.SESSION):
	if auth == Auth.SESSION:
		if token == "":
			yield(get_tree(), "idle_frame")
			return T.Result.failure(T.ZError.new(401, "unauthenticated", "not authenticated"))
		if _expires_at > 0 and _expires_at - int(OS.get_unix_time()) < refresh_leeway_seconds and refresh_token != "":
			var rr = yield(refresh(), "completed")
			if not rr.ok: return rr
	var http = HTTPRequest.new()
	http.timeout = 30
	add_child(http)
	var headers = PoolStringArray(["Accept: application/json", "User-Agent: " + user_agent])
	if auth == Auth.API_KEY or (auth == Auth.SESSION and token == ""): headers.append("x-api-key: " + api_key)
	if auth == Auth.SESSION and token != "": headers.append("Authorization: Bearer " + token)
	var payload = ""
	if body != null:
		headers.append("Content-Type: application/json")
		payload = JSON.print(body)
	var methods = {"GET": HTTPClient.METHOD_GET, "POST": HTTPClient.METHOD_POST, "PUT": HTTPClient.METHOD_PUT, "PATCH": HTTPClient.METHOD_PATCH, "DELETE": HTTPClient.METHOD_DELETE}
	var err = http.request(url + path, headers, true, methods[method], payload)
	if err != OK:
		http.queue_free()
		yield(get_tree(), "idle_frame")
		return T.Result.failure(T.ZError.new(0, "transport", "request error %d" % err))
	var res: Array = yield(http, "request_completed")
	http.queue_free()
	var result: int = res[0]
	var status: int = res[1]
	var text: String = (res[3] as PoolByteArray).get_string_from_utf8()
	if result != HTTPRequest.RESULT_SUCCESS:
		return T.Result.failure(T.ZError.new(0, "transport", "http result %d" % result))
	var parsed = null
	if text != "":
		var p = JSON.parse(text)
		parsed = p.result if p.error == OK else null
	if status < 200 or status >= 300:
		var e = T.ZError.new(status, "http_error", text.left(200))
		if parsed is Dictionary:
			e.code = str(parsed.get("code", e.code)); e.message = str(parsed.get("message", e.message))
		if status == 401 and auth == Auth.SESSION: _clear_session()
		return T.Result.failure(e)
	return T.Result.success(parsed if parsed != null else {})

func _apply_session(s) -> void:
	token = s.token
	refresh_token = s.refresh_token
	_expires_at = _jwt_exp(token)
	emit_signal("session_changed", token, refresh_token)

func _clear_session() -> void:
	token = ""; refresh_token = ""; _expires_at = 0
	emit_signal("session_changed", "", "")

static func _jwt_exp(jwt: String) -> int:
	var parts = jwt.split(".")
	if parts.size() < 2: return 0
	var b64: String = parts[1].replace("-", "+").replace("_", "/")
	while b64.length() % 4 != 0: b64 += "="
	var p = JSON.parse(Marshalls.base64_to_utf8(b64))
	return int(p.result.get("exp", 0)) if p.error == OK and p.result is Dictionary else 0

static func _names_q(names: PoolStringArray) -> String:
	return "?names=" + names.join(",").percent_encode() if names.size() > 0 else ""

static func _now_iso() -> String:
	var d = OS.get_datetime(true)
	return "%04d-%02d-%02dT%02d:%02d:%02dZ" % [d.year, d.month, d.day, d.hour, d.minute, d.second]
