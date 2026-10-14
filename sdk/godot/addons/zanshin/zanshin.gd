extends Node
## Zanshin client for one identity. Added as the `Zanshin` autoload by the plugin; every method is
## awaitable and returns `ZanshinTypes.Result` (or a plain value for the *_or helpers).
##
##   Zanshin.configure("https://liveops.example.com", "zk_c_...")
##   var r := await Zanshin.authenticate(player_id, {"platform": OS.get_name().to_lower()})
##   if (await Zanshin.flag_bool("new_shop")): ...

const T := preload("zanshin_types.gd")
const StreamScript := preload("zanshin_stream.gd")

signal session_changed(token: String, refresh_token: String)   ## empty strings = logged out / revoked

var url: String = ""
var api_key: String = ""
var token: String = ""
var refresh_token: String = ""
var refresh_leeway_seconds: int = 30

var _expires_at: int = 0
var _refreshing: bool = false
var _refresh_result: T.Result = null

enum Auth { NONE, API_KEY, SESSION }

func configure(base_url: String, client_api_key: String) -> void:
	url = base_url.rstrip("/")
	api_key = client_api_key

func is_authenticated() -> bool:
	return token != ""

## Re-use persisted tokens; refreshes lazily when the access token is stale.
func restore(access_token: String, p_refresh_token: String) -> void:
	token = access_token
	refresh_token = p_refresh_token
	_expires_at = _jwt_exp(token)

# ---- session ------------------------------------------------------------------------------------

func authenticate(id: String, default_props: Dictionary = {}, custom_props: Dictionary = {}) -> T.Result:
	var body := {"id": id}
	if not default_props.is_empty(): body["default"] = default_props
	if not custom_props.is_empty(): body["custom"] = custom_props
	var r := await _request("POST", "/v1/authenticate", body, Auth.API_KEY)
	if r.ok:
		var s := T.Session.new(r.value)
		_apply_session(s)
		return T.Result.success(s)
	return r

func refresh() -> T.Result:
	if _refreshing:
		while _refreshing: await get_tree().process_frame
		return _refresh_result
	if refresh_token == "":
		return T.Result.failure(T.ZError.new(401, "unauthenticated", "no refresh token"))
	_refreshing = true
	var r := await _request("POST", "/v1/authenticate/refresh", {"refresh_token": refresh_token}, Auth.API_KEY)
	if r.ok:
		var s := T.Session.new(r.value)
		_apply_session(s)
		_refresh_result = T.Result.success(s)
	else:
		if r.err.status == 401: _clear_session()
		_refresh_result = r
	_refreshing = false
	return _refresh_result

func logout() -> T.Result:
	if token == "": return T.Result.success(null)
	var body := {"token": token, "refresh_token": refresh_token}
	_clear_session()
	return await _request("POST", "/v1/authenticate/logout", body, Auth.API_KEY)

# ---- identity & properties ----------------------------------------------------------------------

func get_identity() -> T.Result: return await _request("GET", "/v1/identity")
func delete_identity() -> T.Result:
	var r := await _request("DELETE", "/v1/identity")
	if r.ok: _clear_session()
	return r
func get_properties() -> T.Result: return await _request("GET", "/v1/properties")
func update_properties(default_props: Dictionary = {}, custom_props: Dictionary = {}) -> T.Result:
	var body := {}
	if not default_props.is_empty(): body["default"] = default_props
	if not custom_props.is_empty(): body["custom"] = custom_props
	return await _request("PUT", "/v1/properties", body)

# ---- flags --------------------------------------------------------------------------------------

func flags(names: PackedStringArray = []) -> T.Result:
	var r := await _request("GET", "/v1/flags" + _names_q(names))
	return T.Result.success(T.map(r.value.get("flags"), func(d): return T.Flag.new(d))) if r.ok else r

func flag(key: String) -> T.Result:
	var r := await _request("GET", "/v1/flags/" + key.uri_encode())
	return T.Result.success(T.Flag.new(r.value)) if r.ok else r

func flag_bool(key: String, fallback: bool = false) -> bool:
	var r := await flag(key)
	return r.value.as_bool(fallback) if r.ok else fallback
func flag_string(key: String, fallback: String = "") -> String:
	var r := await flag(key)
	return r.value.as_string(fallback) if r.ok else fallback
func flag_number(key: String, fallback: float = 0.0) -> float:
	var r := await flag(key)
	return r.value.as_number(fallback) if r.ok else fallback

func audiences() -> T.Result:
	var r := await _request("GET", "/v1/audiences")
	return T.Result.success(Array(r.value.get("audiences", []))) if r.ok else r

# ---- experiments --------------------------------------------------------------------------------

func experiments(names: PackedStringArray = []) -> T.Result:
	var r := await _request("GET", "/v1/experiments" + _names_q(names))
	return T.Result.success(T.map(r.value.get("experiments"), func(d): return T.Experiment.new(d))) if r.ok else r

func expose(experiment: String) -> T.Result:
	return await _request("POST", "/v1/experiments/" + experiment.uri_encode() + "/expose")

# ---- live events / inbox ------------------------------------------------------------------------

func live_events(names: PackedStringArray = [], upcoming: bool = false) -> T.Result:
	var q := _names_q(names)
	if upcoming: q += ("&" if q != "" else "?") + "upcoming=true"
	var r := await _request("GET", "/v1/live-events" + q)
	return T.Result.success(T.map(r.value.get("live_events"), func(d): return T.LiveEvent.new(d))) if r.ok else r

func inbox(limit: int = 0) -> T.Result:
	var r := await _request("GET", "/v1/messages" + ("?limit=%d" % limit if limit > 0 else ""))
	return T.Result.success(T.map(r.value.get("messages"), func(d): return T.InboxMessage.new(d))) if r.ok else r

func mark_read(id: String) -> T.Result:
	var r := await _request("PUT", "/v1/messages/" + id.uri_encode(), {"read": true})
	return T.Result.success(T.InboxMessage.new(r.value)) if r.ok else r

func consume(id: String) -> T.Result:
	return await _request("DELETE", "/v1/messages/" + id.uri_encode())

# ---- push / analytics ---------------------------------------------------------------------------

func register_push_token(platform: String, push_token: String, device_id: String = "") -> T.Result:
	var body := {"platform": platform, "token": push_token}
	if device_id != "": body["device_id"] = device_id
	return await _request("PUT", "/v1/push-tokens", body)

func unregister_push_token(push_token: String) -> T.Result:
	return await _request("DELETE", "/v1/push-tokens/" + push_token.uri_encode())

## Report events: each item is {"name": String, "value": Variant (optional), "metadata": Dictionary (optional)}.
## Returns the number of accepted events.
func track_events(events: Array) -> T.Result:
	var list: Array = []
	for e in events:
		var o := {"name": e.get("name", ""), "timestamp": Time.get_datetime_string_from_system(true) + "Z"}
		if e.has("value") and e["value"] != null:
			o["value"] = e["value"] if e["value"] is String else JSON.stringify(e["value"])
		if e.get("metadata") is Dictionary and not e["metadata"].is_empty(): o["metadata"] = e["metadata"]
		list.append(o)
	var r := await _request("POST", "/v1/events", {"events": list})
	return T.Result.success(int(r.value.get("accepted", 0))) if r.ok else r

func track(name: String, value: Variant = null, metadata: Dictionary = {}) -> T.Result:
	return await track_events([{"name": name, "value": value, "metadata": metadata}])

## Live WebSocket stream (a child Node you can connect signals to).
func stream(flag_names: PackedStringArray = []) -> Node:
	var s: Node = StreamScript.new()
	s.setup(self, flag_names)
	add_child(s)
	return s

# ---- transport ----------------------------------------------------------------------------------

func _request(method: String, path: String, body: Variant = null, auth: Auth = Auth.SESSION) -> T.Result:
	if auth == Auth.SESSION:
		if token == "":
			return T.Result.failure(T.ZError.new(401, "unauthenticated", "not authenticated"))
		if _expires_at > 0 and _expires_at - int(Time.get_unix_time_from_system()) < refresh_leeway_seconds and refresh_token != "":
			var rr := await refresh()
			if not rr.ok: return rr
	var http := HTTPRequest.new()
	http.timeout = 30.0
	add_child(http)
	var headers := PackedStringArray(["Accept: application/json", "User-Agent: zanshin-godot/0.1"])
	if auth == Auth.API_KEY or (auth == Auth.SESSION and token == ""): headers.append("x-api-key: " + api_key)
	if auth == Auth.SESSION and token != "": headers.append("Authorization: Bearer " + token)
	var payload := ""
	if body != null:
		headers.append("Content-Type: application/json")
		payload = JSON.stringify(body)
	var m: int = {"GET": HTTPClient.METHOD_GET, "POST": HTTPClient.METHOD_POST, "PUT": HTTPClient.METHOD_PUT, "PATCH": HTTPClient.METHOD_PATCH, "DELETE": HTTPClient.METHOD_DELETE}[method]
	var err := http.request(url + path, headers, m, payload)
	if err != OK:
		http.queue_free()
		return T.Result.failure(T.ZError.new(0, "transport", "request error %d" % err))
	var res: Array = await http.request_completed
	http.queue_free()
	var result: int = res[0]
	var status: int = res[1]
	var text := (res[3] as PackedByteArray).get_string_from_utf8()
	if result != HTTPRequest.RESULT_SUCCESS:
		return T.Result.failure(T.ZError.new(0, "transport", "http result %d" % result))
	var parsed: Variant = JSON.parse_string(text) if text != "" else {}
	if status < 200 or status >= 300:
		var e := T.ZError.new(status, "http_error", text.left(200))
		if parsed is Dictionary:
			e.code = str(parsed.get("code", e.code)); e.message = str(parsed.get("message", e.message))
		if status == 401 and auth == Auth.SESSION: _clear_session()
		return T.Result.failure(e)
	return T.Result.success(parsed if parsed != null else {})

func _apply_session(s: T.Session) -> void:
	token = s.token
	refresh_token = s.refresh_token
	_expires_at = _jwt_exp(token)
	session_changed.emit(token, refresh_token)

func _clear_session() -> void:
	token = ""; refresh_token = ""; _expires_at = 0
	session_changed.emit("", "")

static func _jwt_exp(jwt: String) -> int:
	var parts := jwt.split(".")
	if parts.size() < 2: return 0
	var b64 := parts[1].replace("-", "+").replace("_", "/")
	while b64.length() % 4 != 0: b64 += "="
	var payload: Variant = JSON.parse_string(Marshalls.base64_to_utf8(b64))
	return int(payload.get("exp", 0)) if payload is Dictionary else 0

static func _names_q(names: PackedStringArray) -> String:
	return "?names=" + ",".join(names).uri_encode() if names.size() > 0 else ""
