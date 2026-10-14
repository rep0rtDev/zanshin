import type { ZanshinClient } from './client.js';
import type { Experiment, Flag, InboxMessage, LiveEvent, StreamMessage } from './types.js';

/** Minimal WebSocket surface (browser WebSocket, Node 22+ global, or the `ws` package). */
export interface WebSocketLike {
  readonly readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}

export type WebSocketFactory = (url: string) => WebSocketLike;

export interface StreamOptions {
  /** Only receive changes for these flags (experiments / live events / inbox are always pushed). */
  names?: string[];
  /** WebSocket constructor; defaults to the global `WebSocket`. */
  webSocket?: WebSocketFactory;
  /** Reconnect with exponential backoff (default true). */
  reconnect?: boolean;
  onOpen?: () => void;
  onClose?: () => void;
  onError?: (err: unknown) => void;
}

type Handlers = {
  flag: Set<(flag: Flag) => void>;
  flagDeleted: Set<(key: string) => void>;
  flags: Set<(flags: Flag[]) => void>;
  experiments: Set<(e: Experiment[]) => void>;
  liveEvents: Set<(e: LiveEvent[]) => void>;
  inbox: Set<(m: InboxMessage[]) => void>;
  message: Set<(m: StreamMessage) => void>;
};

/**
 * Live change stream for one session. Keeps a local cache of the latest evaluations and
 * re-authenticates the socket after token refresh.
 *
 * ```ts
 * const s = z.stream();
 * s.onFlag((f) => applyFlag(f));
 * s.connect();
 * ```
 */
export class ZanshinStream {
  private ws: WebSocketLike | null = null;
  private closed = false;
  private attempt = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private readonly handlers: Handlers = {
    flag: new Set(),
    flagDeleted: new Set(),
    flags: new Set(),
    experiments: new Set(),
    liveEvents: new Set(),
    inbox: new Set(),
    message: new Set(),
  };

  /** Latest known flags by key (updated from the stream). */
  readonly flags = new Map<string, Flag>();

  constructor(
    private readonly client: ZanshinClient,
    private readonly opts: StreamOptions = {},
  ) {}

  onFlag(h: (flag: Flag) => void): () => void {
    this.handlers.flag.add(h);
    return () => this.handlers.flag.delete(h);
  }
  onFlagDeleted(h: (key: string) => void): () => void {
    this.handlers.flagDeleted.add(h);
    return () => this.handlers.flagDeleted.delete(h);
  }
  onFlags(h: (flags: Flag[]) => void): () => void {
    this.handlers.flags.add(h);
    return () => this.handlers.flags.delete(h);
  }
  onExperiments(h: (e: Experiment[]) => void): () => void {
    this.handlers.experiments.add(h);
    return () => this.handlers.experiments.delete(h);
  }
  onLiveEvents(h: (e: LiveEvent[]) => void): () => void {
    this.handlers.liveEvents.add(h);
    return () => this.handlers.liveEvents.delete(h);
  }
  onInbox(h: (m: InboxMessage[]) => void): () => void {
    this.handlers.inbox.add(h);
    return () => this.handlers.inbox.delete(h);
  }
  /** Every raw message. */
  onMessage(h: (m: StreamMessage) => void): () => void {
    this.handlers.message.add(h);
    return () => this.handlers.message.delete(h);
  }

  get connected(): boolean {
    return this.ws !== null && this.ws.readyState === 1;
  }

  private wsUrl(): string {
    const token = this.client.token;
    if (!token) throw new Error('zanshin: authenticate before connecting the stream');
    const base = this.client.url.replace(/^http/, 'ws');
    const q = new URLSearchParams({ token });
    if (this.opts.names?.length) q.set('names', this.opts.names.join(','));
    return `${base}/v1/stream?${q.toString()}`;
  }

  connect(): void {
    this.closed = false;
    this.open();
  }

  private open(): void {
    if (this.closed) return;
    const factory: WebSocketFactory = this.opts.webSocket ?? ((u) => new (globalThis as unknown as { WebSocket: new (u: string) => WebSocketLike }).WebSocket(u));
    let ws: WebSocketLike;
    try {
      ws = factory(this.wsUrl());
    } catch (e) {
      this.opts.onError?.(e);
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      this.attempt = 0;
      this.opts.onOpen?.();
      this.pingTimer = setInterval(() => {
        if (ws.readyState === 1) ws.send('{"type":"ping"}');
      }, 30_000);
    };
    ws.onmessage = (ev) => {
      let msg: StreamMessage;
      try {
        msg = JSON.parse(typeof ev.data === 'string' ? ev.data : String(ev.data)) as StreamMessage;
      } catch {
        return;
      }
      this.dispatch(msg);
    };
    ws.onerror = (e) => this.opts.onError?.(e);
    ws.onclose = () => {
      if (this.pingTimer) clearInterval(this.pingTimer);
      this.pingTimer = null;
      this.ws = null;
      this.opts.onClose?.();
      this.scheduleReconnect();
    };
  }

  private dispatch(msg: StreamMessage): void {
    switch (msg.type) {
      case 'flag':
        this.flags.set(msg.flag.key, msg.flag);
        this.handlers.flag.forEach((h) => h(msg.flag));
        break;
      case 'flag_deleted':
        this.flags.delete(msg.key);
        this.handlers.flagDeleted.forEach((h) => h(msg.key));
        break;
      case 'flags':
        for (const f of msg.flags) this.flags.set(f.key, f);
        this.handlers.flags.forEach((h) => h(msg.flags));
        break;
      case 'experiments':
        this.handlers.experiments.forEach((h) => h(msg.experiments));
        break;
      case 'live_events':
        this.handlers.liveEvents.forEach((h) => h(msg.live_events));
        break;
      case 'inbox':
        this.handlers.inbox.forEach((h) => h(msg.messages));
        break;
      case 'error':
        // Expired session: refresh and reconnect with the new token.
        if (/expired/i.test(msg.message)) {
          void this.client.refresh().catch((e) => this.opts.onError?.(e));
        }
        break;
      default:
        break;
    }
    this.handlers.message.forEach((h) => h(msg));
  }

  private scheduleReconnect(): void {
    if (this.closed || this.opts.reconnect === false) return;
    const delay = Math.min(30_000, 1000 * 2 ** Math.min(this.attempt, 5)) * (0.5 + Math.random());
    this.attempt++;
    this.timer = setTimeout(async () => {
      this.timer = null;
      if (!this.client.token) {
        try {
          await this.client.refresh();
        } catch (e) {
          this.opts.onError?.(e);
          this.scheduleReconnect();
          return;
        }
      }
      this.open();
    }, delay);
  }

  /** Stop and do not reconnect. */
  close(): void {
    this.closed = true;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    if (this.pingTimer) clearInterval(this.pingTimer);
    this.pingTimer = null;
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      ws.onclose = null;
      ws.close(1000, 'bye');
    }
  }
}
