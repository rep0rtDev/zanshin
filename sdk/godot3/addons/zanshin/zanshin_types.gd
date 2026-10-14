class_name ZanshinTypes
# Typed views over the JSON the server returns (Godot 3.x). Every class keeps the raw Dictionary in `raw`.

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

# Result of an async call: `ok`, `value`, `err`.
class Result:
	var ok: bool = false
	var value = null
	var err = null
	static func success(v) -> Result:
		var r = Result.new(); r.ok = true; r.value = v; return r
	static func failure(e) -> Result:
		var r = Result.new(); r.ok = false; r.err = e; return r

class Flag:
	var raw: Dictionary
	var key: String
	var type: String
	var value
	var variant: String
	var reason: String
	var version: int
	func _init(d: Dictionary = {}) -> void:
		raw = d
		key = str(d.get("key", ""))
		type = str(d.get("value_type", ""))
		value = d.get("value")
		variant = str(d.get("variant", "")) if d.get("variant") != null else ""
		var r = d.get("reason")
		reason = str(r.get("kind", "")) if r is Dictionary else ""
		version = int(d.get("version", 0))
	func as_bool(fallback: bool = false) -> bool:
		match typeof(value):
			TYPE_BOOL: return value
			TYPE_STRING: return (value as String).to_lower() == "true"
			TYPE_INT, TYPE_REAL: return value != 0
		return fallback
	func as_string(fallback: String = "") -> String:
		if value == null: return fallback
		return value if value is String else str(value)
	func as_number(fallback: float = 0.0) -> float:
		match typeof(value):
			TYPE_INT, TYPE_REAL: return float(value)
			TYPE_STRING: return float(value) if (value as String).is_valid_float() else fallback
		return fallback
	func as_dict(fallback: Dictionary = {}) -> Dictionary:
		return value if value is Dictionary else fallback

class Experiment:
	var raw: Dictionary
	var key: String
	var variant: String
	var value
	var exposed: bool
	func _init(d: Dictionary = {}) -> void:
		raw = d; key = str(d.get("key", "")); variant = str(d.get("variant", "")); value = d.get("value"); exposed = bool(d.get("exposed", false))

class LiveEvent:
	var raw: Dictionary
	var key: String
	var name: String
	var payload: Dictionary
	var active: bool
	var current: Dictionary  # {start, end} ISO strings or empty
	var next: Dictionary
	func _init(d: Dictionary = {}) -> void:
		raw = d; key = str(d.get("key", "")); name = str(d.get("name", ""))
		payload = d.get("payload") if d.get("payload") is Dictionary else {}
		active = bool(d.get("active", false))
		current = d.get("current") if d.get("current") is Dictionary else {}
		next = d.get("next") if d.get("next") is Dictionary else {}

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
		raw = d; id = str(d.get("id", "")); key = str(d.get("key", "")) if d.get("key") != null else ""
		title = str(d.get("title", "")); body = str(d.get("body", "")); image_url = str(d.get("image_url", ""))
		payload = d.get("payload") if d.get("payload") is Dictionary else {}
		priority = int(d.get("priority", 0)); delivered_at = str(d.get("delivered_at", ""))
		read = d.get("read_at") != null; expires_at = str(d.get("expires_at", "")) if d.get("expires_at") != null else ""

class Session:
	var raw: Dictionary
	var token: String
	var refresh_token: String
	var identity_id: String
	var external_id: String
	func _init(d: Dictionary = {}) -> void:
		raw = d; token = str(d.get("token", "")); refresh_token = str(d.get("refresh_token", ""))
		var i = d.get("identity", {})
		identity_id = str(i.get("id", "")) if i is Dictionary else ""
		external_id = str(i.get("external_id", "")) if i is Dictionary else ""

# Map an Array of Dictionaries onto one of the classes above by name ("Flag", "Experiment", ...).
static func map(arr, type_name: String) -> Array:
	var out: Array = []
	if arr is Array:
		for it in arr:
			if it is Dictionary:
				match type_name:
					"Flag": out.append(Flag.new(it))
					"Experiment": out.append(Experiment.new(it))
					"LiveEvent": out.append(LiveEvent.new(it))
					"InboxMessage": out.append(InboxMessage.new(it))
	return out
