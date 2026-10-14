class_name ZanshinTypes
## Typed views over the JSON the server returns. Every class keeps the raw Dictionary in `raw`.

class ZError:
	var status: int = 0
	var code: String = ""
	var message: String = ""
	func _init(s: int = 0, c: String = "", m: String = "") -> void:
		status = s; code = c; message = m
	func is_not_found() -> bool: return status == 404
	func is_unauthenticated() -> bool: return status == 401
	func is_rate_limited() -> bool: return status == 429
	func _to_string() -> String: return "%d %s: %s" % [status, code, message]

## Result of an async call: `ok`, `value`, `error`.
class Result:
	var ok: bool = false
	var value: Variant = null
	var err: ZError = null
	static func success(v: Variant) -> Result:
		var r := Result.new(); r.ok = true; r.value = v; return r
	static func failure(e: ZError) -> Result:
		var r := Result.new(); r.ok = false; r.err = e; return r

class Flag:
	var raw: Dictionary
	var key: String
	var type: String
	var value: Variant
	var variant: String
	var reason: String
	var version: int
	func _init(d: Dictionary = {}) -> void:
		raw = d
		key = d.get("key", "")
		type = d.get("value_type", "")
		value = d.get("value")
		variant = str(d.get("variant", ""))
		var r = d.get("reason")
		reason = r.get("kind", "") if r is Dictionary else ""
		version = int(d.get("version", 0))
	func as_bool(fallback: bool = false) -> bool:
		match typeof(value):
			TYPE_BOOL: return value
			TYPE_STRING: return (value as String).to_lower() == "true"
			TYPE_INT, TYPE_FLOAT: return value != 0
		return fallback
	func as_string(fallback: String = "") -> String:
		if value == null: return fallback
		return value if value is String else str(value)
	func as_number(fallback: float = 0.0) -> float:
		match typeof(value):
			TYPE_INT, TYPE_FLOAT: return float(value)
			TYPE_STRING: return float(value) if (value as String).is_valid_float() else fallback
		return fallback
	func as_dict(fallback: Dictionary = {}) -> Dictionary:
		return value if value is Dictionary else fallback

class Experiment:
	var raw: Dictionary
	var key: String
	var variant: String
	var value: Variant
	var exposed: bool
	func _init(d: Dictionary = {}) -> void:
		raw = d; key = d.get("key", ""); variant = str(d.get("variant", "")); value = d.get("value"); exposed = bool(d.get("exposed", false))

class LiveEvent:
	var raw: Dictionary
	var key: String
	var name: String
	var payload: Dictionary
	var active: bool
	var current: Dictionary  # {start, end} ISO strings or empty
	var next: Dictionary
	func _init(d: Dictionary = {}) -> void:
		raw = d; key = d.get("key", ""); name = d.get("name", "")
		payload = d.get("payload", {}) if d.get("payload") is Dictionary else {}
		active = bool(d.get("active", false))
		current = d.get("current", {}) if d.get("current") is Dictionary else {}
		next = d.get("next", {}) if d.get("next") is Dictionary else {}

class InboxMessage:
	var raw: Dictionary
	var id: String
	var key: String
	var title: String
	var body: String
	var image_url: String
	var payload: Dictionary
	var priority: int
	var delivered_at: String
	var read: bool
	var expires_at: String
	func _init(d: Dictionary = {}) -> void:
		raw = d; id = d.get("id", ""); key = str(d.get("key", "")) if d.get("key") != null else ""
		title = d.get("title", ""); body = d.get("body", ""); image_url = d.get("image_url", "")
		payload = d.get("payload", {}) if d.get("payload") is Dictionary else {}
		priority = int(d.get("priority", 0)); delivered_at = d.get("delivered_at", "")
		read = d.get("read_at") != null; expires_at = str(d.get("expires_at", "")) if d.get("expires_at") != null else ""

class Session:
	var raw: Dictionary
	var token: String
	var refresh_token: String
	var identity_id: String
	var external_id: String
	func _init(d: Dictionary = {}) -> void:
		raw = d; token = d.get("token", ""); refresh_token = d.get("refresh_token", "")
		var i = d.get("identity", {})
		identity_id = i.get("id", "") if i is Dictionary else ""
		external_id = i.get("external_id", "") if i is Dictionary else ""

static func map(arr: Variant, ctor: Callable) -> Array:
	var out: Array = []
	if arr is Array:
		for it in arr:
			if it is Dictionary: out.append(ctor.call(it))
	return out
