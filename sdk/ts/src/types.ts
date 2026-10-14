export type JsonValue = string | number | boolean | null | JsonValue[] | { [k: string]: JsonValue };
export type PropertyMap = Record<string, JsonValue>;

export interface Properties {
  default: PropertyMap;
  computed: PropertyMap;
  custom: PropertyMap;
}

export interface Identity {
  id: string;
  external_id: string;
  created_at: string;
  updated_at: string;
  last_seen_at: string;
}

export interface Session {
  token: string;
  refresh_token: string;
  identity: Identity;
  properties: Properties;
}

export type ReasonKind = 'disabled' | 'default' | 'override' | 'experiment' | 'rule' | 'variant';

export interface Reason {
  kind: ReasonKind;
  rule_id?: string;
  variant?: string;
  experiment?: string;
}

export interface Flag<T = JsonValue> {
  key: string;
  value: T;
  value_type: 'bool' | 'string' | 'number' | 'json';
  variant?: string;
  reason: Reason;
  version: number;
}

export interface Experiment<T = JsonValue> {
  key: string;
  name: string;
  variant: string;
  value: T;
  version: number;
}

export interface Occurrence {
  start: string;
  end: string;
}

export interface LiveEvent<T = JsonValue> {
  key: string;
  name: string;
  description: string;
  value: T;
  active: boolean;
  current?: Occurrence;
  next?: Occurrence;
  version: number;
}

export interface InboxMessage<T = JsonValue> {
  id: string;
  key: string | null;
  title: string;
  body: string;
  image_url: string;
  payload: T;
  priority: number;
  delivered_at: string;
  read_at: string | null;
  expires_at: string | null;
}

export interface EventInput {
  name: string;
  value?: string;
  metadata?: Record<string, JsonValue>;
  /** ISO-8601; omitted = server receipt time. */
  timestamp?: string;
  /** Idempotency key (UUID). */
  id?: string;
}

/** Messages pushed on the `/v1/stream` WebSocket. */
export type StreamMessage =
  | { type: 'hello'; identity_id: string }
  | { type: 'flag'; flag: Flag }
  | { type: 'flag_deleted'; key: string }
  | { type: 'flags'; flags: Flag[] }
  | { type: 'experiments'; experiments: Experiment[] }
  | { type: 'live_events'; live_events: LiveEvent[] }
  | { type: 'inbox'; messages: InboxMessage[] }
  | { type: 'pong' }
  | { type: 'error'; message: string };

export class ZanshinError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'ZanshinError';
  }
  get isNotFound(): boolean {
    return this.status === 404;
  }
  get isUnauthenticated(): boolean {
    return this.status === 401;
  }
}
