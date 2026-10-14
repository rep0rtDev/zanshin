--- Zanshin client SDK for Defold (Lua 5.1). Zero dependencies beyond Defold's built-in `http`,
--- `json` and (optionally) `crypt` / `extension-websocket`. Every call is asynchronous and reports
--- through a callback `cb(ok, value_or_error)`; errors are tables `{status, code, message}`.
---
---   local zanshin = require "zanshin.zanshin"
---   local z = zanshin.new({ url = "https://liveops.example.com", api_key = "zk_c_..." })
---   z:authenticate(player_id, { default = { platform = sys.get_sys_info().system_name } }, function(ok, s)
---     if ok then z:flag_bool("new_shop", false, function(on) ... end) end
---   end)
local M = {}
M.__index = M
M.VERSION = "0.1.0"

local function now()
	if socket and socket.gettime then return socket.gettime() end
	return os.time()
end

local function iso_now()
	return os.date("!%Y-%m-%dT%H:%M:%SZ")
end

local function url_encode(s)
	return (tostring(s):gsub("[^%w%-_%.~]", function(c) return string.format("%%%02X", c:byte()) end))
end

local function b64_decode(s)
	if crypt and crypt.decode_base64 then return crypt.decode_base64(s) end
	local b = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
	s = s:gsub("[^" .. b .. "=]", "")
	return (s:gsub(".", function(x)
		if x == "=" then return "" end
		local r, f = "", (b:find(x, 1, true) - 1)
		for i = 6, 1, -1 do r = r .. (f % 2 ^ i - f % 2 ^ (i - 1) > 0 and "1" or "0") end
		return r
	end):gsub("%d%d%d?%d?%d?%d?%d?%d?", function(x)
		if #x ~= 8 then return "" end
		local c = 0
		for i = 1, 8 do c = c + (x:sub(i, i) == "1" and 2 ^ (8 - i) or 0) end
		return string.char(c)
	end))
end

local function jwt_exp(token)
	local payload = token:match("^[^%.]+%.([^%.]+)")
	if not payload then return 0 end
	payload = payload:gsub("-", "+"):gsub("_", "/")
	while #payload % 4 ~= 0 do payload = payload .. "=" end
	local ok, data = pcall(json.decode, b64_decode(payload))
	return ok and type(data) == "table" and tonumber(data.exp) or 0
end

local function err(status, code, message)
	return { status = status or 0, code = code or "error", message = message or "" }
end

--- Create a client. opts: url, api_key, refresh_leeway_seconds (default 30), on_session(token, refresh_token | nil),
--- http (override for tests: function(url, method, cb, headers, body)).
function M.new(opts)
	assert(opts and opts.url and opts.api_key, "zanshin.new needs url and api_key")
	local self = setmetatable({}, M)
	self.url = opts.url:gsub("/+$", "")
	self.api_key = opts.api_key
	self.leeway = opts.refresh_leeway_seconds or 30
	self.on_session = opts.on_session
	self.http = opts.http or function(url, method, cb, headers, body)
		http.request(url, method, function(_, _, response) cb(response) end, headers, body, { timeout = 30 })
	end
	self.token, self.refresh_token, self.expires_at = nil, nil, 0
	self._refresh_waiters = nil
	return self
end

function M:is_authenticated() return self.token ~= nil end

--- Re-use persisted tokens; refreshes lazily when the access token is stale.
function M:restore(token, refresh_token)
	self.token, self.refresh_token, self.expires_at = token, refresh_token, jwt_exp(token)
end

function M:_set_session(s)
	self.token, self.refresh_token, self.expires_at = s.token, s.refresh_token, jwt_exp(s.token or "")
	if self.on_session then self.on_session(self.token, self.refresh_token) end
end

function M:_clear_session()
	self.token, self.refresh_token, self.expires_at = nil, nil, 0
	if self.on_session then self.on_session(nil, nil) end
end

function M:_expiring_soon()
	return self.expires_at > 0 and now() >= self.expires_at - self.leeway
end

--- Low-level request. auth: "key" | "session" | "none".
function M:_request(method, path, body, auth, cb)
	auth = auth or "session"
	if auth == "session" then
		if not self.token then return cb(false, err(401, "unauthenticated", "not authenticated")) end
		if self:_expiring_soon() and self.refresh_token then
			return self:refresh(function(ok, e)
				if not ok then return cb(false, e) end
				self:_request(method, path, body, auth, cb)
			end)
		end
	end
	local headers = { ["Accept"] = "application/json", ["User-Agent"] = "zanshin-defold/" .. M.VERSION }
	if auth == "key" then headers["x-api-key"] = self.api_key end
	if auth == "session" then headers["Authorization"] = "Bearer " .. self.token end
	local payload = nil
	if body ~= nil then
		headers["Content-Type"] = "application/json"
		payload = json.encode(body)
	end
	self.http(self.url .. path, method, function(response)
		local status = response.status or 0
		local text = response.response or ""
		if status == 0 then return cb(false, err(0, "transport", response.error or "network error")) end
		local parsed = nil
		if text ~= "" then
			local ok, v = pcall(json.decode, text)
			if ok then parsed = v end
		end
		if status < 200 or status >= 300 then
			local e = err(status, "http_error", text:sub(1, 200))
			if type(parsed) == "table" then
				e.code = parsed.code or e.code
				e.message = parsed.message or e.message
			end
			if status == 401 and auth == "session" then self:_clear_session() end
			return cb(false, e)
		end
		cb(true, parsed == nil and {} or parsed)
	end, headers, payload)
end

local function names_q(names)
	if names and #names > 0 then return "?names=" .. url_encode(table.concat(names, ",")) end
	return ""
end

-- ---- session ---------------------------------------------------------------------------------

--- props: { default = {...}, custom = {...} }.
function M:authenticate(id, props, cb)
	if type(props) == "function" then cb, props = props, nil end
	props = props or {}
	self:_request("POST", "/v1/authenticate", { id = id, default = props.default or {}, custom = props.custom or {} }, "key", function(ok, s)
		if ok then self:_set_session(s) end
		cb(ok, s)
	end)
end

--- Rotate tokens; concurrent calls share one request.
function M:refresh(cb)
	cb = cb or function() end
	if self._refresh_waiters then table.insert(self._refresh_waiters, cb) return end
	if not self.refresh_token then return cb(false, err(401, "unauthenticated", "no refresh token")) end
	self._refresh_waiters = { cb }
	self:_request("POST", "/v1/authenticate/refresh", { refresh_token = self.refresh_token }, "key", function(ok, s)
		local waiters = self._refresh_waiters
		self._refresh_waiters = nil
		if ok then self:_set_session(s) elseif s.status == 401 then self:_clear_session() end
		for _, w in ipairs(waiters) do w(ok, s) end
	end)
end

function M:logout(cb)
	cb = cb or function() end
	local t, rt = self.token, self.refresh_token
	self:_clear_session()
	if not t then return cb(true, {}) end
	self:_request("POST", "/v1/authenticate/logout", { token = t, refresh_token = rt }, "key", cb)
end

-- ---- identity & properties -------------------------------------------------------------------

function M:identity(cb) self:_request("GET", "/v1/identity", nil, "session", cb) end
function M:delete_identity(cb)
	self:_request("DELETE", "/v1/identity", nil, "session", function(ok, v)
		if ok then self:_clear_session() end
		cb(ok, v)
	end)
end
function M:properties(cb) self:_request("GET", "/v1/properties", nil, "session", cb) end
--- props: { default = {...}, custom = {...} }; json.null removes a key.
function M:update_properties(props, cb)
	self:_request("PUT", "/v1/properties", { default = props.default or {}, custom = props.custom or {} }, "session", cb)
end
--- Link the current (anonymous) identity to `id`; switches to the returned session.
function M:identify(id, cb)
	self:_request("POST", "/v1/identify", { id = id }, "session", function(ok, s)
		if ok then self:_set_session(s) end
		cb(ok, s)
	end)
end

-- ---- flags / audiences / experiments ---------------------------------------------------------

function M:flags(names, cb)
	if type(names) == "function" then cb, names = names, nil end
	self:_request("GET", "/v1/flags" .. names_q(names), nil, "session", function(ok, v) cb(ok, ok and v.flags or v) end)
end
function M:flag(key, cb) self:_request("GET", "/v1/flags/" .. url_encode(key), nil, "session", cb) end

local function coerce_bool(v)
	if type(v) == "boolean" then return v end
	if type(v) == "string" then return v:lower() == "true" end
	if type(v) == "number" then return v ~= 0 end
	return nil
end

--- Typed helpers: cb(value) with `fallback` when the flag is missing or has another type.
function M:flag_bool(key, fallback, cb)
	self:flag(key, function(ok, f)
		local v = nil
		if ok then v = coerce_bool(f.value) end
		if v == nil then v = fallback end
		cb(v)
	end)
end
function M:flag_number(key, fallback, cb)
	self:flag(key, function(ok, f)
		local v = nil
		if ok then v = tonumber(f.value) end
		if v == nil then v = fallback end
		cb(v)
	end)
end
function M:flag_string(key, fallback, cb)
	self:flag(key, function(ok, f)
		if not ok or f.value == nil then return cb(fallback) end
		cb(type(f.value) == "string" and f.value or tostring(f.value))
	end)
end
function M:audiences(cb) self:_request("GET", "/v1/audiences", nil, "session", function(ok, v) cb(ok, ok and v.audiences or v) end) end
function M:experiments(names, cb)
	if type(names) == "function" then cb, names = names, nil end
	self:_request("GET", "/v1/experiments" .. names_q(names), nil, "session", function(ok, v) cb(ok, ok and v.experiments or v) end)
end
function M:expose(experiment, cb) self:_request("POST", "/v1/experiments/" .. url_encode(experiment) .. "/expose", nil, "session", cb or function() end) end

-- ---- live events / inbox ---------------------------------------------------------------------

--- opts: { names = {...}, upcoming = true }.
function M:live_events(opts, cb)
	if type(opts) == "function" then cb, opts = opts, nil end
	opts = opts or {}
	local q = names_q(opts.names)
	if opts.upcoming then q = q .. (q == "" and "?" or "&") .. "upcoming=true" end
	self:_request("GET", "/v1/live-events" .. q, nil, "session", function(ok, v) cb(ok, ok and v.live_events or v) end)
end
function M:join_live_event(key, cb) self:_request("POST", "/v1/live-events/" .. url_encode(key) .. "/join", nil, "session", cb) end
function M:leave_live_event(key, cb)
	self:_request("POST", "/v1/live-events/" .. url_encode(key) .. "/leave", nil, "session", function(ok, v) cb(ok, ok and v.left or v) end)
end
function M:inbox(limit, cb)
	if type(limit) == "function" then cb, limit = limit, nil end
	self:_request("GET", "/v1/messages" .. (limit and ("?limit=" .. limit) or ""), nil, "session", function(ok, v) cb(ok, ok and v.messages or v) end)
end
function M:mark_read(id, cb) self:_request("PUT", "/v1/messages/" .. url_encode(id), { read = true }, "session", cb) end
function M:consume(id, cb) self:_request("DELETE", "/v1/messages/" .. url_encode(id), nil, "session", cb or function() end) end
function M:delete_inbox_message(id, cb) self:_request("DELETE", "/v1/inbox/" .. url_encode(id), nil, "session", cb or function() end) end

-- ---- push tokens / analytics -----------------------------------------------------------------

function M:register_push_token(platform, token, device_id, cb)
	if type(device_id) == "function" then cb, device_id = device_id, nil end
	self:_request("PUT", "/v1/push-tokens", { platform = platform, token = token, device_id = device_id or "" }, "session", cb or function() end)
end
function M:unregister_push_token(token, cb) self:_request("DELETE", "/v1/push-tokens/" .. url_encode(token), nil, "session", cb or function() end) end

--- events: list of { name = "...", value = any, metadata = {...} }. cb(ok, accepted_count).
function M:track_events(events, cb)
	local list = {}
	for _, e in ipairs(events) do
		local o = { name = e.name, timestamp = e.timestamp or iso_now() }
		if e.value ~= nil then o.value = type(e.value) == "string" and e.value or json.encode(e.value) end
		if e.metadata and next(e.metadata) ~= nil then o.metadata = e.metadata end
		if e.id then o.id = e.id end
		list[#list + 1] = o
	end
	self:_request("POST", "/v1/events", { events = list }, "session", function(ok, v) if cb then cb(ok, ok and v.accepted or v) end end)
end
function M:track(name, value, metadata, cb)
	if type(value) == "function" then cb, value, metadata = value, nil, nil
	elseif type(metadata) == "function" then cb, metadata = metadata, nil end
	self:track_events({ { name = name, value = value, metadata = metadata } }, cb)
end

-- ---- live stream (requires defold extension-websocket) ----------------------------------------

--- Open /v1/stream. handlers: on_flag(flag), on_flag_deleted(key), on_flags(list), on_experiments(list),
--- on_live_events(list), on_inbox(list), on_open(), on_close(), on_error(err). names: optional flag filter.
--- Returns a handle with :close(). Reconnects with jittered backoff; refreshes the session when it expires.
function M:stream(handlers, names)
	assert(websocket, "zanshin stream needs the Defold websocket extension (github.com/defold/extension-websocket)")
	handlers = handlers or {}
	local h = { closed = false, attempt = 0, conn = nil, flags = {} }
	local function schedule(fn, delay)
		if timer and timer.delay then timer.delay(delay, false, fn) else fn() end
	end
	local open
	local function reconnect()
		if h.closed then return end
		local base = math.min(30, 0.5 * 2 ^ math.min(h.attempt, 6))
		h.attempt = h.attempt + 1
		schedule(open, base + math.random() * base * 0.3)
	end
	open = function()
		if h.closed then return end
		if not self.token then
			if handlers.on_error then handlers.on_error(err(401, "unauthenticated", "stream needs a session")) end
			return reconnect()
		end
		local base = self.url:gsub("^https://", "wss://"):gsub("^http://", "ws://")
		local q = "token=" .. url_encode(self.token)
		if names and #names > 0 then q = q .. "&names=" .. url_encode(table.concat(names, ",")) end
		h.conn = websocket.connect(base .. "/v1/stream?" .. q, {}, function(_, _, data)
			if data.event == websocket.EVENT_CONNECTED then
				h.attempt = 0
				if handlers.on_open then handlers.on_open() end
			elseif data.event == websocket.EVENT_DISCONNECTED or data.event == websocket.EVENT_ERROR then
				h.conn = nil
				if handlers.on_close then handlers.on_close() end
				if data.event == websocket.EVENT_ERROR and handlers.on_error then handlers.on_error(err(0, "transport", data.message or "websocket error")) end
				if not h.closed then
					if self.refresh_token then self:refresh(function() reconnect() end) else reconnect() end
				end
			elseif data.event == websocket.EVENT_MESSAGE then
				local ok, m = pcall(json.decode, data.message or "")
				if ok and type(m) == "table" then
					if m.type == "flag" and m.flag then
						h.flags[m.flag.key] = m.flag
						if handlers.on_flag then handlers.on_flag(m.flag) end
					elseif m.type == "flag_deleted" then
						h.flags[m.key] = nil
						if handlers.on_flag_deleted then handlers.on_flag_deleted(m.key) end
					elseif m.type == "flags" then
						for _, f in ipairs(m.flags or {}) do h.flags[f.key] = f end
						if handlers.on_flags then handlers.on_flags(m.flags or {}) end
					elseif m.type == "experiments" and handlers.on_experiments then handlers.on_experiments(m.experiments or {})
					elseif m.type == "live_events" and handlers.on_live_events then handlers.on_live_events(m.live_events or {})
					elseif m.type == "inbox" and handlers.on_inbox then handlers.on_inbox(m.messages or {})
					elseif m.type == "error" and handlers.on_error then handlers.on_error(err(0, m.code or "error", m.message or "")) end
				end
			end
		end)
	end
	function h.close()
		h.closed = true
		if h.conn then websocket.disconnect(h.conn) h.conn = nil end
	end
	function h.ping()
		if h.conn then websocket.send(h.conn, '{"type":"ping"}') end
	end
	open()
	return h
end

return M
