-- Zanshin LiveOps for the Nakama Lua runtime.
--
-- Copy this file into Nakama's `data/modules/` and set the runtime environment:
--   runtime.env:
--     - "ZANSHIN_URL=http://zanshin:7550"
--     - "ZANSHIN_SERVER_KEY=zk_s_..."
--
-- Nakama users become Zanshin identities on login (after-authenticate hooks); clients call the
-- same RPCs as with the Go and TypeScript modules: zanshin.flags, zanshin.flag,
-- zanshin.flag_explain, zanshin.audiences, zanshin.experiments, zanshin.expose,
-- zanshin.live_events, zanshin.live_event_join, zanshin.live_event_leave, zanshin.inbox,
-- zanshin.inbox_read, zanshin.inbox_delete, zanshin.track, zanshin.properties and
-- (server-to-server only) zanshin.audience_members.

local nk = require("nakama")

local TIMEOUT_MS = 5000

local function config(context)
  local env = context.env or {}
  local url = (env["ZANSHIN_URL"] or "http://127.0.0.1:7550"):gsub("/+$", "")
  local key = env["ZANSHIN_SERVER_KEY"]
  if not key or key == "" then
    error({ message = "ZANSHIN_SERVER_KEY is not configured", code = 13 })
  end
  return url, key
end

local function url_encode(s)
  return (tostring(s):gsub("[^%w%-%._~]", function(c)
    return string.format("%%%02X", string.byte(c))
  end))
end

local function query(params)
  local parts = {}
  for k, v in pairs(params) do
    if v ~= nil and v ~= "" then
      parts[#parts + 1] = url_encode(k) .. "=" .. url_encode(v)
    end
  end
  table.sort(parts)
  if #parts == 0 then
    return ""
  end
  return "?" .. table.concat(parts, "&")
end

local function grpc_code(status)
  local map = { [400] = 3, [401] = 16, [403] = 7, [404] = 5, [409] = 6, [429] = 8, [503] = 14 }
  return map[status] or 13
end

-- Calls the Zanshin server API; raises a Nakama runtime error with the matching gRPC code.
local function call(context, method, path, body)
  local base, key = config(context)
  local headers = { ["x-api-key"] = key, ["accept"] = "application/json" }
  local payload = nil
  if body ~= nil then
    headers["content-type"] = "application/json"
    payload = nk.json_encode(body)
  end
  local ok, code, _, resp = pcall(nk.http_request, base .. path, method, headers, payload, TIMEOUT_MS)
  if not ok then
    error({ message = "zanshin unreachable: " .. tostring(code), code = 14 })
  end
  if code >= 200 and code < 300 then
    if resp == nil or resp == "" then
      return {}
    end
    return nk.json_decode(resp)
  end
  local message = resp
  local parsed_ok, parsed = pcall(nk.json_decode, resp or "")
  if parsed_ok and type(parsed) == "table" and parsed.message then
    message = parsed.message
  end
  error({ message = message, code = grpc_code(code) })
end

local function user(context)
  if not context.user_id or context.user_id == "" then
    error({ message = "no user in context", code = 16 })
  end
  return context.user_id
end

local function parse(payload)
  if payload == nil or payload == "" or not payload:match("%S") then
    return {}
  end
  local ok, p = pcall(nk.json_decode, payload)
  if not ok or type(p) ~= "table" then
    error({ message = "payload must be JSON", code = 3 })
  end
  return p
end

local function names(p)
  if p.names and #p.names > 0 then
    return table.concat(p.names, ",")
  end
  return nil
end

local function key_of(p, field)
  local k = p.key or p.name
  if not k or k == "" then
    error({ message = field .. " is required", code = 3 })
  end
  return tostring(k)
end

-- --- identity sync ---------------------------------------------------------------------------

local function after_auth(provider)
  return function(context, _payload)
    local uid = context.user_id
    if not uid or uid == "" then
      return
    end
    local props = { auth_provider = provider }
    if context.client_ip and context.client_ip ~= "" then
      props.client_ip = context.client_ip
    end
    if context.lang and context.lang ~= "" then
      props.lang = context.lang
    end
    local ok, accounts = pcall(nk.accounts_get_id, { uid })
    if ok and accounts and accounts[1] and accounts[1].user then
      local u = accounts[1].user
      props.username = u.username
      if u.lang_tag and u.lang_tag ~= "" then props.lang = u.lang_tag end
      if u.location and u.location ~= "" then props.location = u.location end
      if u.timezone and u.timezone ~= "" then props.timezone = u.timezone end
      if u.create_time then props.created_at = os.date("!%Y-%m-%dT%H:%M:%SZ", u.create_time) end
    end
    local sync_ok, err = pcall(call, context, "POST", "/server/v1/identities", { external_id = uid, default = props, custom = {} })
    if not sync_ok then
      nk.logger_warn(string.format("zanshin: identity sync failed for %s: %s", uid, nk.json_encode(err)))
    end
  end
end

-- --- RPCs ------------------------------------------------------------------------------------

local rpcs = {}

rpcs["zanshin.flags"] = function(context, payload)
  local uid = user(context)
  local p = parse(payload)
  return nk.json_encode(call(context, "GET", "/server/v1/flags" .. query({ external_id = uid, names = names(p) })))
end

rpcs["zanshin.flag"] = function(context, payload)
  local uid = user(context)
  local key = key_of(parse(payload), "name")
  return nk.json_encode(call(context, "GET", "/server/v1/flags/" .. url_encode(key) .. query({ external_id = uid })))
end

rpcs["zanshin.flag_explain"] = function(context, payload)
  local uid = user(context)
  local key = key_of(parse(payload), "name")
  return nk.json_encode(call(context, "GET", "/server/v1/flags/" .. url_encode(key) .. "/explain" .. query({ external_id = uid })))
end

rpcs["zanshin.audiences"] = function(context, _payload)
  local uid = user(context)
  return nk.json_encode(call(context, "GET", "/server/v1/audiences" .. query({ external_id = uid })))
end

rpcs["zanshin.experiments"] = function(context, payload)
  local uid = user(context)
  local p = parse(payload)
  return nk.json_encode(call(context, "GET", "/server/v1/experiments" .. query({ external_id = uid, names = names(p) })))
end

rpcs["zanshin.expose"] = function(context, payload)
  local uid = user(context)
  local p = parse(payload)
  if not p.experiment or p.experiment == "" then
    error({ message = "experiment is required", code = 3 })
  end
  call(context, "POST", "/server/v1/experiments/" .. url_encode(p.experiment) .. "/expose" .. query({ external_id = uid }))
  return "{}"
end

rpcs["zanshin.live_events"] = function(context, payload)
  local uid = user(context)
  local p = parse(payload)
  local upcoming = nil
  if p.upcoming then upcoming = "true" end
  return nk.json_encode(call(context, "GET", "/server/v1/live-events" .. query({ external_id = uid, names = names(p), upcoming = upcoming })))
end

rpcs["zanshin.live_event_join"] = function(context, payload)
  local uid = user(context)
  local key = key_of(parse(payload), "key")
  return nk.json_encode(call(context, "POST", "/server/v1/live-events/" .. url_encode(key) .. "/join" .. query({ external_id = uid })))
end

rpcs["zanshin.live_event_leave"] = function(context, payload)
  local uid = user(context)
  local key = key_of(parse(payload), "key")
  return nk.json_encode(call(context, "POST", "/server/v1/live-events/" .. url_encode(key) .. "/leave" .. query({ external_id = uid })))
end

rpcs["zanshin.inbox"] = function(context, payload)
  local uid = user(context)
  local p = parse(payload)
  local limit = nil
  if p.limit then limit = tostring(p.limit) end
  return nk.json_encode(call(context, "GET", "/server/v1/messages/inbox" .. query({ external_id = uid, limit = limit })))
end

rpcs["zanshin.inbox_read"] = function(context, payload)
  local uid = user(context)
  local p = parse(payload)
  if not p.id or p.id == "" then
    error({ message = "id is required", code = 3 })
  end
  local id = url_encode(p.id)
  if p.consume then
    call(context, "POST", "/server/v1/messages/inbox/" .. id .. "/consume" .. query({ external_id = uid }))
    return nk.json_encode({ consumed = p.id })
  end
  return nk.json_encode(call(context, "PUT", "/server/v1/messages/inbox/" .. id .. query({ external_id = uid }), { read = true }))
end

rpcs["zanshin.inbox_delete"] = function(context, payload)
  local uid = user(context)
  local p = parse(payload)
  if not p.id or p.id == "" then
    error({ message = "id is required", code = 3 })
  end
  call(context, "DELETE", "/server/v1/messages/inbox/" .. url_encode(p.id) .. query({ external_id = uid }))
  return nk.json_encode({ deleted = p.id })
end

rpcs["zanshin.track"] = function(context, payload)
  local uid = user(context)
  local p = parse(payload)
  if not p.events or #p.events == 0 then
    error({ message = "events are required", code = 3 })
  end
  for _, e in ipairs(p.events) do
    local name = tostring(e.name or "")
    if name:sub(1, 1) == "$" then
      error({ message = string.format("event name %q is reserved", name), code = 3 })
    end
  end
  return nk.json_encode(call(context, "POST", "/server/v1/events", { external_id = uid, events = p.events }))
end

rpcs["zanshin.properties"] = function(context, payload)
  local uid = user(context)
  local p = parse(payload)
  if not p.custom or next(p.custom) == nil then
    error({ message = "custom properties are required", code = 3 })
  end
  return nk.json_encode(call(context, "PUT", "/server/v1/identities/" .. url_encode(uid) .. "/properties", { custom = p.custom }))
end

-- Server-to-server only (Nakama HTTP key, no user in context).
rpcs["zanshin.audience_members"] = function(context, payload)
  if context.user_id and context.user_id ~= "" then
    error({ message = "server-to-server only", code = 7 })
  end
  local p = parse(payload)
  if not p.audience or p.audience == "" then
    error({ message = "audience is required", code = 3 })
  end
  local path = "/server/v1/audiences/" .. url_encode(p.audience) .. "/members"
  local add = p.add or {}
  local remove = p.remove or {}
  if #add > 0 then call(context, "POST", path, { external_ids = add }) end
  if #remove > 0 then call(context, "DELETE", path, { external_ids = remove }) end
  return nk.json_encode({ added = #add, removed = #remove })
end

for id, fn in pairs(rpcs) do
  nk.register_rpc(fn, id)
end

for _, provider in ipairs({ "device", "email", "custom", "google", "apple", "steam" }) do
  nk.register_req_after(after_auth(provider), "authenticate" .. provider)
end

nk.logger_info("zanshin module loaded")
