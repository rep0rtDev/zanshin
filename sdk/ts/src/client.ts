import {
  EventInput,
  Experiment,
  Flag,
  InboxMessage,
  JsonValue,
  LiveEvent,
  Properties,
  PropertyMap,
  Session,
  ZanshinError,
} from './types.js';

export interface ClientOptions {
  /** REST base URL, e.g. `https://liveops.example.com` (no trailing slash needed). */
  url: string;
  /** Client API key. */
  apiKey: string;
  /** Custom fetch (defaults to the global one). */
  fetch?: typeof fetch;
  /** Called whenever tokens change (persist them to resume the session later). */
  onSession?: (session: { token: string; refreshToken: string } | null) => void;
  /** Refresh the access token this many seconds before it expires (default 60). */
  refreshLeewaySeconds?: number;
}

interface Tokens {
  token: string;
  refreshToken: string;
  expiresAt: number; // unix seconds
}

function decodeExp(jwt: string): number {
  try {
    const payload = jwt.split('.')[1] ?? '';
    const b64 = payload.replace(/-/g, '+').replace(/_/g, '/');
    const g = globalThis as unknown as { atob?: (s: string) => string; Buffer?: { from(s: string, e: string): { toString(e: string): string } } };
    const json = g.atob ? g.atob(b64) : g.Buffer ? g.Buffer.from(b64, 'base64').toString('utf8') : '{}';
    const exp = (JSON.parse(json) as { exp?: number }).exp;
    return typeof exp === 'number' ? exp : 0;
  } catch {
    return 0;
  }
}

/**
 * Zanshin client for a single identity (one player / device).
 *
 * ```ts
 * const z = new ZanshinClient({ url, apiKey });
 * await z.authenticate(deviceId, { default: { platform: 'web' } });
 * const shop = await z.flag<boolean>('shop_v2');
 * ```
 */
export class ZanshinClient {
  private readonly base: string;
  private readonly apiKey: string;
  private readonly fetchImpl: typeof fetch;
  private readonly onSession: ClientOptions['onSession'];
  private readonly leeway: number;
  private tokens: Tokens | null = null;
  private refreshing: Promise<Session> | null = null;

  constructor(opts: ClientOptions) {
    this.base = opts.url.replace(/\/+$/, '');
    this.apiKey = opts.apiKey;
    this.fetchImpl = opts.fetch ?? globalThis.fetch.bind(globalThis);
    this.onSession = opts.onSession;
    this.leeway = opts.refreshLeewaySeconds ?? 60;
  }

  /** Base URL of the server. */
  get url(): string {
    return this.base;
  }

  /** Current access token, or null when not authenticated. */
  get token(): string | null {
    return this.tokens?.token ?? null;
  }

  get isAuthenticated(): boolean {
    return this.tokens !== null;
  }

  /** Resume a persisted session; refreshes immediately if the access token is stale. */
  async restore(token: string, refreshToken: string): Promise<void> {
    this.setTokens(token, refreshToken, false);
    if (this.expiringSoon()) {
      await this.refresh();
    }
  }

  private setTokens(token: string, refreshToken: string, notify = true): void {
    this.tokens = { token, refreshToken, expiresAt: decodeExp(token) };
    if (notify) this.onSession?.({ token, refreshToken });
  }

  private expiringSoon(): boolean {
    if (!this.tokens) return true;
    if (this.tokens.expiresAt === 0) return false;
    return Date.now() / 1000 >= this.tokens.expiresAt - this.leeway;
  }

  // ---- transport ---------------------------------------------------------------------------

  private async request<T>(
    method: string,
    path: string,
    opts: { query?: Record<string, string | undefined>; body?: unknown; auth?: 'key' | 'session' | 'none' } = {},
  ): Promise<T> {
    let url = this.base + path;
    if (opts.query) {
      const q = new URLSearchParams();
      for (const [k, v] of Object.entries(opts.query)) if (v !== undefined && v !== '') q.set(k, v);
      const s = q.toString();
      if (s) url += '?' + s;
    }
    const auth = opts.auth ?? 'session';
    const headers: Record<string, string> = { accept: 'application/json' };
    if (opts.body !== undefined) headers['content-type'] = 'application/json';
    if (auth === 'key') headers['x-api-key'] = this.apiKey;
    if (auth === 'session') {
      if (!this.tokens) throw new ZanshinError(401, 'unauthenticated', 'not authenticated');
      if (this.expiringSoon() && this.tokens.refreshToken) await this.refresh();
      headers['authorization'] = 'Bearer ' + this.tokens.token;
    }
    const res = await this.fetchImpl(url, { method, headers, body: opts.body === undefined ? null : JSON.stringify(opts.body) });
    const text = await res.text();
    if (!res.ok) {
      let code = res.statusText || 'error';
      let message = text;
      try {
        const e = JSON.parse(text) as { code?: string; message?: string };
        if (e.code) code = e.code;
        if (e.message) message = e.message;
      } catch {
        /* non-JSON error body */
      }
      if (res.status === 401 && auth === 'session') {
        this.tokens = null;
        this.onSession?.(null);
      }
      throw new ZanshinError(res.status, code, message);
    }
    return (text ? JSON.parse(text) : {}) as T;
  }

  // ---- session -----------------------------------------------------------------------------

  /** Authenticate (create or resume) the identity `id` and open a session. */
  async authenticate(id: string, props: { default?: PropertyMap; custom?: PropertyMap } = {}): Promise<Session> {
    const s = await this.request<Session>('POST', '/v1/authenticate', { auth: 'key', body: { id, default: props.default ?? {}, custom: props.custom ?? {} } });
    this.setTokens(s.token, s.refresh_token);
    return s;
  }

  /** Rotate tokens. Concurrent calls share one request. */
  refresh(): Promise<Session> {
    if (this.refreshing) return this.refreshing;
    const rt = this.tokens?.refreshToken;
    if (!rt) return Promise.reject(new ZanshinError(401, 'unauthenticated', 'no refresh token'));
    this.refreshing = this.request<Session>('POST', '/v1/authenticate/refresh', { auth: 'key', body: { refresh_token: rt } })
      .then((s) => {
        this.setTokens(s.token, s.refresh_token);
        return s;
      })
      .catch((e) => {
        if (e instanceof ZanshinError && e.status === 401) {
          this.tokens = null;
          this.onSession?.(null);
        }
        throw e;
      })
      .finally(() => {
        this.refreshing = null;
      });
    return this.refreshing;
  }

  /** Revoke the session server-side and forget tokens. */
  async logout(): Promise<void> {
    const t = this.tokens;
    this.tokens = null;
    this.onSession?.(null);
    if (!t) return;
    await this.request('POST', '/v1/authenticate/logout', { auth: 'key', body: { token: t.token, refresh_token: t.refreshToken } });
  }

  // ---- identity & properties ---------------------------------------------------------------

  identity(): Promise<import('./types.js').Identity> {
    return this.request('GET', '/v1/identity');
  }

  /** Delete the identity and everything attached to it. */
  async deleteIdentity(): Promise<void> {
    await this.request('DELETE', '/v1/identity');
    this.tokens = null;
    this.onSession?.(null);
  }

  properties(): Promise<Properties> {
    return this.request('GET', '/v1/properties');
  }

  /** Merge properties; a `null` value removes the key. */
  updateProperties(props: { default?: PropertyMap; custom?: PropertyMap }): Promise<Properties> {
    return this.request('PUT', '/v1/properties', { body: { default: props.default ?? {}, custom: props.custom ?? {} } });
  }

  // ---- flags -------------------------------------------------------------------------------

  async flags(names?: string[]): Promise<Flag[]> {
    const r = await this.request<{ flags: Flag[] }>('GET', '/v1/flags', { query: { names: names?.join(',') } });
    return r.flags;
  }

  flag<T = JsonValue>(key: string): Promise<Flag<T>> {
    return this.request('GET', '/v1/flags/' + encodeURIComponent(key));
  }

  /** Flag value with a fallback when the flag does not exist. */
  async flagValue<T = JsonValue>(key: string, fallback: T): Promise<T> {
    try {
      return (await this.flag<T>(key)).value;
    } catch (e) {
      if (e instanceof ZanshinError && e.isNotFound) return fallback;
      throw e;
    }
  }

  async audiences(): Promise<string[]> {
    return (await this.request<{ audiences: string[] }>('GET', '/v1/audiences')).audiences;
  }

  // ---- experiments -------------------------------------------------------------------------

  async experiments(names?: string[]): Promise<Experiment[]> {
    return (await this.request<{ experiments: Experiment[] }>('GET', '/v1/experiments', { query: { names: names?.join(',') } })).experiments;
  }

  /** Record that the identity actually saw its variant. */
  async expose(experiment: string): Promise<void> {
    await this.request('POST', '/v1/experiments/' + encodeURIComponent(experiment) + '/expose');
  }

  // ---- live events -------------------------------------------------------------------------

  async liveEvents(opts: { names?: string[]; upcoming?: boolean } = {}): Promise<LiveEvent[]> {
    const r = await this.request<{ live_events: LiveEvent[] }>('GET', '/v1/live-events', {
      query: { names: opts.names?.join(','), upcoming: opts.upcoming ? 'true' : undefined },
    });
    return r.live_events;
  }

  // ---- inbox -------------------------------------------------------------------------------

  async inbox(limit?: number): Promise<InboxMessage[]> {
    return (await this.request<{ messages: InboxMessage[] }>('GET', '/v1/messages', { query: { limit: limit?.toString() } })).messages;
  }

  markRead(id: string): Promise<InboxMessage> {
    return this.request('PUT', '/v1/messages/' + encodeURIComponent(id), { body: { read: true } });
  }

  /** Consume (remove) a message; it is never redelivered. */
  async consume(id: string): Promise<void> {
    await this.request('DELETE', '/v1/messages/' + encodeURIComponent(id));
  }

  // ---- analytics ---------------------------------------------------------------------------

  /** Report events; returns how many were accepted. */
  async track(...events: EventInput[]): Promise<number> {
    if (events.length === 0) return 0;
    return (await this.request<{ accepted: number }>('POST', '/v1/events', { body: { events } })).accepted;
  }

  /** Convenience: one event. */
  event(name: string, value?: string | number, metadata?: Record<string, JsonValue>): Promise<number> {
    const e: EventInput = { name };
    if (value !== undefined) e.value = String(value);
    if (metadata) e.metadata = metadata;
    return this.track(e);
  }
}
