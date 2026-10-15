/**
 * OpenFeature provider for Zanshin. Structural types only — no dependency on `@openfeature/*`;
 * the object satisfies `Provider` from `@openfeature/server-sdk` (and `web-sdk`).
 *
 *   OpenFeature.setProvider(new ZanshinProvider({ client, local: true }));
 *   const c = OpenFeature.getClient();
 *   await c.getBooleanValue('shop', false, { targetingKey: 'player-42', custom: { level: 7 } });
 *
 * `targetingKey` is the player's external id. Other context attributes become `custom.*`
 * properties for local evaluation (nested `default` / `custom` objects are honoured as-is);
 * remote evaluation uses the identity's stored properties.
 */
import type { ZanshinClient } from './client.js';
import { LocalEvaluator, flatten, type FlatProps } from './local.js';
import type { Flag, JsonValue } from './types.js';

export type EvaluationContext = { targetingKey?: string; [key: string]: unknown };

export interface ResolutionDetails<T> {
  value: T;
  variant?: string;
  reason?: string;
  errorCode?: string;
  errorMessage?: string;
  flagMetadata?: Record<string, string | number | boolean>;
}

export interface ZanshinProviderOptions {
  client: ZanshinClient;
  /** Evaluate in-process from the rules bundle (server key). Default: remote via `/server/v1/flags`. */
  local?: boolean;
  /** Bundle refresh interval in local mode (ms, default 60 000). */
  refreshMs?: number;
  /** Remote mode: how to fetch flags for a player (defaults to `client.serverFlags`). */
  fetchFlags?: (externalId: string, names: string[]) => Promise<Flag[]>;
}

const REASONS: Record<string, string> = {
  default: 'DEFAULT', disabled: 'DISABLED', rule: 'TARGETING_MATCH', variant: 'SPLIT', experiment: 'SPLIT',
  override: 'TARGETING_MATCH', live_event: 'TARGETING_MATCH', prerequisite: 'DEFAULT',
};

export class ZanshinProvider {
  readonly metadata = { name: 'zanshin' };
  readonly runsOn = 'server' as const;
  readonly hooks = [];
  private evaluator?: LocalEvaluator;
  private loadedAt = 0;
  private loading: Promise<LocalEvaluator> | undefined;

  constructor(private readonly opts: ZanshinProviderOptions) {}

  async initialize(): Promise<void> {
    if (this.opts.local) await this.rules();
  }

  async onClose(): Promise<void> {}

  private async rules(): Promise<LocalEvaluator> {
    const ttl = this.opts.refreshMs ?? 60_000;
    if (this.evaluator && Date.now() - this.loadedAt < ttl) return this.evaluator;
    if (!this.loading) {
      this.loading = this.opts.client.rules().then((b) => {
        this.evaluator = new LocalEvaluator(b);
        this.loadedAt = Date.now();
        return this.evaluator;
      }).catch((e) => {
        if (this.evaluator) return this.evaluator;
        throw e;
      }).finally(() => { this.loading = undefined; });
    }
    return this.loading;
  }

  private static props(ctx: EvaluationContext): FlatProps {
    const { targetingKey, ...rest } = ctx;
    const custom: Record<string, JsonValue> = {};
    const groups: Record<string, Record<string, JsonValue>> = {};
    for (const [k, v] of Object.entries(rest)) {
      if ((k === 'default' || k === 'computed' || k === 'custom') && v && typeof v === 'object' && !Array.isArray(v)) groups[k] = v as Record<string, JsonValue>;
      else if (v !== undefined) custom[k] = v as JsonValue;
    }
    return flatten(targetingKey ?? '', { ...groups, custom: { ...(groups['custom'] ?? {}), ...custom } });
  }

  private async resolve(flagKey: string, ctx: EvaluationContext): Promise<Flag | undefined> {
    const subject = ctx.targetingKey ?? '';
    if (this.opts.local) return (await this.rules()).evaluate(flagKey, subject, ZanshinProvider.props(ctx));
    if (!subject) throw new Error('targetingKey (player external id) is required for remote evaluation');
    const fetch = this.opts.fetchFlags ?? ((id, names) => this.opts.client.serverFlags(id, names));
    return (await fetch(subject, [flagKey])).find((f) => f.key === flagKey);
  }

  private async typed<T>(flagKey: string, defaultValue: T, ctx: EvaluationContext, accept: (v: unknown) => v is T): Promise<ResolutionDetails<T>> {
    let f: Flag | undefined;
    try {
      f = await this.resolve(flagKey, ctx);
    } catch (e) {
      return { value: defaultValue, reason: 'ERROR', errorCode: 'GENERAL', errorMessage: e instanceof Error ? e.message : String(e) };
    }
    if (!f) return { value: defaultValue, reason: 'ERROR', errorCode: 'FLAG_NOT_FOUND', errorMessage: `flag ${flagKey} not found` };
    if (!accept(f.value)) return { value: defaultValue, reason: 'ERROR', errorCode: 'TYPE_MISMATCH', errorMessage: `flag ${flagKey} is ${f.value_type}` };
    const out: ResolutionDetails<T> = { value: f.value, reason: REASONS[f.reason.kind] ?? 'UNKNOWN', flagMetadata: { version: f.version, reason: f.reason.kind } };
    if (f.variant) out.variant = f.variant;
    if (f.reason.rule_id) out.flagMetadata!['rule_id'] = f.reason.rule_id;
    return out;
  }

  resolveBooleanEvaluation(flagKey: string, defaultValue: boolean, ctx: EvaluationContext = {}): Promise<ResolutionDetails<boolean>> {
    return this.typed(flagKey, defaultValue, ctx, (v): v is boolean => typeof v === 'boolean');
  }
  resolveStringEvaluation(flagKey: string, defaultValue: string, ctx: EvaluationContext = {}): Promise<ResolutionDetails<string>> {
    return this.typed(flagKey, defaultValue, ctx, (v): v is string => typeof v === 'string');
  }
  resolveNumberEvaluation(flagKey: string, defaultValue: number, ctx: EvaluationContext = {}): Promise<ResolutionDetails<number>> {
    return this.typed(flagKey, defaultValue, ctx, (v): v is number => typeof v === 'number');
  }
  resolveObjectEvaluation<T extends JsonValue>(flagKey: string, defaultValue: T, ctx: EvaluationContext = {}): Promise<ResolutionDetails<T>> {
    return this.typed(flagKey, defaultValue, ctx, (v): v is T => v !== null && typeof v === 'object');
  }
}
