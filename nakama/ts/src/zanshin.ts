// Zanshin LiveOps for the Nakama JavaScript runtime.
//
// Drop `build/zanshin.js` into Nakama's `data/modules/` and set the runtime environment:
//   runtime.env:
//     - "ZANSHIN_URL=http://zanshin:7550"
//     - "ZANSHIN_SERVER_KEY=zk_s_..."
//
// Nakama users become Zanshin identities on login (after-authenticate hooks); clients call the
// same RPCs as with the Go plugin: zanshin.flags, zanshin.flag, zanshin.flag_explain,
// zanshin.audiences, zanshin.experiments, zanshin.expose, zanshin.live_events,
// zanshin.live_event_join, zanshin.live_event_leave, zanshin.inbox, zanshin.inbox_read,
// zanshin.inbox_delete, zanshin.track, zanshin.properties, and (server-to-server only)
// zanshin.audience_members.

const ZANSHIN_TIMEOUT_MS = 5000;

interface ZanshinConfig {
  url: string;
  key: string;
}

interface ZanshinHttpError {
  status: number;
  code: string;
  message: string;
}

function zanshinConfig(ctx: nkruntime.Context): ZanshinConfig {
  const url = (ctx.env['ZANSHIN_URL'] || 'http://127.0.0.1:7550').replace(/\/+$/, '');
  const key = ctx.env['ZANSHIN_SERVER_KEY'] || '';
  if (!key) {
    throw { message: 'ZANSHIN_SERVER_KEY is not configured', code: 13 } as nkruntime.Error;
  }
  return { url: url, key: key };
}

function zanshinQuery(params: { [k: string]: string | undefined }): string {
  const parts: string[] = [];
  for (const k in params) {
    const v = params[k];
    if (v !== undefined && v !== '') parts.push(encodeURIComponent(k) + '=' + encodeURIComponent(v));
  }
  return parts.length ? '?' + parts.join('&') : '';
}

// Calls the Zanshin server API; throws a Nakama runtime error with the matching gRPC code.
function zanshinCall(ctx: nkruntime.Context, nk: nkruntime.Nakama, method: nkruntime.RequestMethod, path: string, body?: unknown): any {
  const cfg = zanshinConfig(ctx);
  const headers: { [k: string]: string } = { 'x-api-key': cfg.key, accept: 'application/json' };
  let payload: string | undefined;
  if (body !== undefined) {
    headers['content-type'] = 'application/json';
    payload = JSON.stringify(body);
  }
  let res: nkruntime.HttpResponse;
  try {
    res = nk.httpRequest(cfg.url + path, method, headers, payload, ZANSHIN_TIMEOUT_MS);
  } catch (e) {
    throw { message: 'zanshin unreachable: ' + String(e), code: 14 } as nkruntime.Error;
  }
  if (res.code >= 200 && res.code < 300) {
    return res.body ? JSON.parse(res.body) : {};
  }
  let err: ZanshinHttpError = { status: res.code, code: 'error', message: res.body };
  try {
    const parsed = JSON.parse(res.body);
    if (parsed && parsed.message) err = { status: res.code, code: parsed.code || 'error', message: parsed.message };
  } catch (_e) {
    /* non-JSON error body */
  }
  throw { message: err.message, code: zanshinGrpcCode(err.status) } as nkruntime.Error;
}

function zanshinGrpcCode(status: number): nkruntime.Codes {
  switch (status) {
    case 400:
      return 3;
    case 401:
      return 16;
    case 403:
      return 7;
    case 404:
      return 5;
    case 409:
      return 6;
    case 429:
      return 8;
    case 503:
      return 14;
    default:
      return 13;
  }
}

function zanshinUser(ctx: nkruntime.Context): string {
  if (!ctx.userId) {
    throw { message: 'no user in context', code: 16 } as nkruntime.Error;
  }
  return ctx.userId;
}

function zanshinParse(payload: string): any {
  if (!payload || !payload.replace(/\s/g, '')) return {};
  try {
    return JSON.parse(payload);
  } catch (_e) {
    throw { message: 'payload must be JSON', code: 3 } as nkruntime.Error;
  }
}

function zanshinNames(p: any): string | undefined {
  return p && p.names && p.names.length ? (p.names as string[]).join(',') : undefined;
}

function zanshinKey(p: any, field: string): string {
  const k = p && (p.key || p.name);
  if (!k) throw { message: field + ' is required', code: 3 } as nkruntime.Error;
  return String(k);
}

// --- identity sync --------------------------------------------------------------------------

function zanshinAfterAuth(provider: string): nkruntime.AfterHookFunction<nkruntime.Session, unknown> {
  return function (ctx, logger, nk) {
    if (!ctx.userId) return;
    const props: { [k: string]: unknown } = { auth_provider: provider };
    if (ctx.clientIp) props['client_ip'] = ctx.clientIp;
    if (ctx.lang) props['lang'] = ctx.lang;
    try {
      const acc = nk.accountGetId(ctx.userId);
      if (acc && acc.user) {
        props['username'] = acc.user.username;
        if (acc.user.langTag) props['lang'] = acc.user.langTag;
        if (acc.user.location) props['location'] = acc.user.location;
        if (acc.user.timezone) props['timezone'] = acc.user.timezone;
        if (acc.user.createTime) props['created_at'] = new Date(acc.user.createTime * 1000).toISOString();
      }
    } catch (_e) {
      /* account lookup is best effort */
    }
    try {
      zanshinCall(ctx, nk, 'post', '/server/v1/identities', { external_id: ctx.userId, default: props, custom: {} });
    } catch (e) {
      logger.warn('zanshin: identity sync failed for %s: %s', ctx.userId, JSON.stringify(e));
    }
  };
}

// --- RPCs -----------------------------------------------------------------------------------

const zanshinRpcs: { [id: string]: nkruntime.RpcFunction } = {
  'zanshin.flags': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const p = zanshinParse(payload);
    return JSON.stringify(zanshinCall(ctx, nk, 'get', '/server/v1/flags' + zanshinQuery({ external_id: uid, names: zanshinNames(p) })));
  },
  'zanshin.flag': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const key = zanshinKey(zanshinParse(payload), 'name');
    return JSON.stringify(zanshinCall(ctx, nk, 'get', '/server/v1/flags/' + encodeURIComponent(key) + zanshinQuery({ external_id: uid })));
  },
  'zanshin.flag_explain': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const key = zanshinKey(zanshinParse(payload), 'name');
    return JSON.stringify(zanshinCall(ctx, nk, 'get', '/server/v1/flags/' + encodeURIComponent(key) + '/explain' + zanshinQuery({ external_id: uid })));
  },
  'zanshin.audiences': function (ctx, _logger, nk) {
    const uid = zanshinUser(ctx);
    return JSON.stringify(zanshinCall(ctx, nk, 'get', '/server/v1/audiences' + zanshinQuery({ external_id: uid })));
  },
  'zanshin.experiments': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const p = zanshinParse(payload);
    return JSON.stringify(zanshinCall(ctx, nk, 'get', '/server/v1/experiments' + zanshinQuery({ external_id: uid, names: zanshinNames(p) })));
  },
  'zanshin.expose': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const p = zanshinParse(payload);
    if (!p.experiment) throw { message: 'experiment is required', code: 3 } as nkruntime.Error;
    zanshinCall(ctx, nk, 'post', '/server/v1/experiments/' + encodeURIComponent(String(p.experiment)) + '/expose' + zanshinQuery({ external_id: uid }));
    return '{}';
  },
  'zanshin.live_events': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const p = zanshinParse(payload);
    return JSON.stringify(zanshinCall(ctx, nk, 'get', '/server/v1/live-events' + zanshinQuery({ external_id: uid, names: zanshinNames(p), upcoming: p.upcoming ? 'true' : undefined })));
  },
  'zanshin.live_event_join': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const key = zanshinKey(zanshinParse(payload), 'key');
    return JSON.stringify(zanshinCall(ctx, nk, 'post', '/server/v1/live-events/' + encodeURIComponent(key) + '/join' + zanshinQuery({ external_id: uid })));
  },
  'zanshin.live_event_leave': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const key = zanshinKey(zanshinParse(payload), 'key');
    return JSON.stringify(zanshinCall(ctx, nk, 'post', '/server/v1/live-events/' + encodeURIComponent(key) + '/leave' + zanshinQuery({ external_id: uid })));
  },
  'zanshin.inbox': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const p = zanshinParse(payload);
    return JSON.stringify(zanshinCall(ctx, nk, 'get', '/server/v1/messages/inbox' + zanshinQuery({ external_id: uid, limit: p.limit ? String(p.limit) : undefined })));
  },
  'zanshin.inbox_read': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const p = zanshinParse(payload);
    if (!p.id) throw { message: 'id is required', code: 3 } as nkruntime.Error;
    const id = encodeURIComponent(String(p.id));
    if (p.consume) {
      zanshinCall(ctx, nk, 'post', '/server/v1/messages/inbox/' + id + '/consume' + zanshinQuery({ external_id: uid }));
      return JSON.stringify({ consumed: p.id });
    }
    return JSON.stringify(zanshinCall(ctx, nk, 'put', '/server/v1/messages/inbox/' + id + zanshinQuery({ external_id: uid }), { read: true }));
  },
  'zanshin.inbox_delete': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const p = zanshinParse(payload);
    if (!p.id) throw { message: 'id is required', code: 3 } as nkruntime.Error;
    zanshinCall(ctx, nk, 'delete', '/server/v1/messages/inbox/' + encodeURIComponent(String(p.id)) + zanshinQuery({ external_id: uid }));
    return JSON.stringify({ deleted: p.id });
  },
  'zanshin.track': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const p = zanshinParse(payload);
    if (!p.events || !p.events.length) throw { message: 'events are required', code: 3 } as nkruntime.Error;
    for (let i = 0; i < p.events.length; i++) {
      const name = String(p.events[i].name || '');
      if (name.charAt(0) === '$') throw { message: 'event name "' + name + '" is reserved', code: 3 } as nkruntime.Error;
    }
    return JSON.stringify(zanshinCall(ctx, nk, 'post', '/server/v1/events', { external_id: uid, events: p.events }));
  },
  'zanshin.properties': function (ctx, _logger, nk, payload) {
    const uid = zanshinUser(ctx);
    const p = zanshinParse(payload);
    if (!p.custom) throw { message: 'custom properties are required', code: 3 } as nkruntime.Error;
    return JSON.stringify(zanshinCall(ctx, nk, 'put', '/server/v1/identities/' + encodeURIComponent(uid) + '/properties', { custom: p.custom }));
  },
  // Server-to-server only (Nakama HTTP key, no user in context).
  'zanshin.audience_members': function (ctx, _logger, nk, payload) {
    if (ctx.userId) throw { message: 'server-to-server only', code: 7 } as nkruntime.Error;
    const p = zanshinParse(payload);
    if (!p.audience) throw { message: 'audience is required', code: 3 } as nkruntime.Error;
    const path = '/server/v1/audiences/' + encodeURIComponent(String(p.audience)) + '/members';
    const add: string[] = p.add || [];
    const remove: string[] = p.remove || [];
    if (add.length) zanshinCall(ctx, nk, 'post', path, { external_ids: add });
    if (remove.length) zanshinCall(ctx, nk, 'delete', path, { external_ids: remove });
    return JSON.stringify({ added: add.length, removed: remove.length });
  },
};

// Nakama looks for a global `InitModule`.
function InitModule(ctx: nkruntime.Context, logger: nkruntime.Logger, _nk: nkruntime.Nakama, initializer: nkruntime.Initializer): void {
  for (const id in zanshinRpcs) {
    initializer.registerRpc(id, zanshinRpcs[id]);
  }
  initializer.registerAfterAuthenticateDevice(zanshinAfterAuth('device'));
  initializer.registerAfterAuthenticateEmail(zanshinAfterAuth('email'));
  initializer.registerAfterAuthenticateCustom(zanshinAfterAuth('custom'));
  initializer.registerAfterAuthenticateGoogle(zanshinAfterAuth('google'));
  initializer.registerAfterAuthenticateApple(zanshinAfterAuth('apple'));
  initializer.registerAfterAuthenticateSteam(zanshinAfterAuth('steam'));
  logger.info('zanshin module loaded (server %s)', ctx.env['ZANSHIN_URL'] || 'http://127.0.0.1:7550');
}

// Keep the symbol referenced so the ES5 single-file output does not drop it.
!InitModule && InitModule;
