/**
 * In-process flag evaluation from a rules bundle (`GET /server/v1/rules`), bit-for-bit
 * compatible with the server: same xxh3 bucketing, same condition operators, same reasons.
 * Overrides, experiments and live events are server-side and are not applied here.
 */
import type { Flag, JsonValue } from './types.js';
import { inRollout, weightedPick } from './xxh3.js';

export interface RulesBundle {
  generated_at: string;
  flags: RuleFlag[];
  audiences: RuleAudience[];
  managed_audiences: string[];
}

export interface RuleFlag {
  key: string;
  value_type: Flag['value_type'];
  default_value: JsonValue;
  enabled: boolean;
  rules: Rule[];
  salt: string;
  prerequisites?: { flag: string; value?: JsonValue }[];
  version: number;
}

export interface Rule {
  id: string;
  condition: Condition;
  audiences?: string[];
  percentage?: number;
  value?: JsonValue;
  variants?: { key: string; value: JsonValue; weight: number }[];
}

export interface RuleAudience {
  key: string;
  condition: Condition;
  percentage?: number;
  salt: string;
  enabled: boolean;
}

export type Condition =
  | { all: Condition[] }
  | { any: Condition[] }
  | { not: Condition }
  | { prop: string; op: string; value?: JsonValue };

export type FlatProps = Record<string, JsonValue | undefined>;

/** `{ default: {...}, custom: {...} }` → `{ "default.k": v, ... , "identity.external_id": id }`. */
export function flatten(externalId: string, props?: Partial<Record<'default' | 'computed' | 'custom', Record<string, JsonValue>>>): FlatProps {
  const flat: FlatProps = {};
  for (const ns of ['default', 'computed', 'custom'] as const) {
    for (const [k, v] of Object.entries(props?.[ns] ?? {})) flat[`${ns}.${k}`] = v;
  }
  flat['identity.external_id'] = externalId;
  return flat;
}

const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const asFloat = (v: unknown): number | undefined => {
  if (isNum(v)) return v;
  if (typeof v === 'string') {
    const t = v.trim();
    if (t === '') return undefined;
    const n = Number(t);
    return Number.isFinite(n) ? n : undefined;
  }
  if (typeof v === 'boolean') return v ? 1 : 0;
  return undefined;
};
const parseBool = (s: string): boolean | undefined => (s === 'true' ? true : s === 'false' ? false : undefined);

function looseEq(a: unknown, b: unknown): boolean {
  if (isNum(a) && isNum(b)) return a === b;
  if (typeof a === 'string' && isNum(b)) return asFloat(a) === b;
  if (isNum(a) && typeof b === 'string') return a === asFloat(b);
  if (typeof a === 'string' && typeof b === 'boolean') return parseBool(a) === b;
  if (typeof a === 'boolean' && typeof b === 'string') return parseBool(b) === a;
  return JSON.stringify(a) === JSON.stringify(b);
}

function compare(a: unknown, b: unknown): number | undefined {
  const fa = asFloat(a), fb = asFloat(b);
  if (typeof a === 'string' && typeof b === 'string' && (fa === undefined || fb === undefined)) return a < b ? -1 : a > b ? 1 : 0;
  if (fa === undefined || fb === undefined) return undefined;
  return fa < fb ? -1 : fa > fb ? 1 : 0;
}

function contains(actual: unknown, needle: unknown): boolean {
  if (typeof actual === 'string') return typeof needle === 'string' && actual.includes(needle);
  if (Array.isArray(actual)) return actual.some((v) => looseEq(v, needle));
  if (actual && typeof actual === 'object') return typeof needle === 'string' && Object.prototype.hasOwnProperty.call(actual, needle);
  return false;
}

function parseSemver(s: string): [number, number, number, string] | undefined {
  let t = s.trim().replace(/^[vV]/, '');
  let pre = '';
  const m = t.match(/^([^-+]*)(?:-([^+]*))?(?:\+.*)?$/);
  if (!m) return undefined;
  t = m[1]!;
  pre = m[2] ?? '';
  const parts = t.split('.');
  while (parts.length < 3) parts.push('0');
  if (parts.length > 3) return undefined;
  const nums = parts.map((p) => (/^\d+$/.test(p) ? Number(p) : NaN));
  if (nums.some((n) => Number.isNaN(n))) return undefined;
  return [nums[0]!, nums[1]!, nums[2]!, pre];
}

function semverCmp(a: unknown, b: unknown): number | undefined {
  if (typeof a !== 'string' || typeof b !== 'string') return undefined;
  const va = parseSemver(a), vb = parseSemver(b);
  if (!va || !vb) return undefined;
  for (let i = 0; i < 3; i++) if (va[i] !== vb[i]) return (va[i] as number) < (vb[i] as number) ? -1 : 1;
  if (va[3] === vb[3]) return 0;
  if (va[3] === '') return 1;
  if (vb[3] === '') return -1;
  return va[3] < vb[3] ? -1 : 1;
}

function ageSecs(v: unknown): number | undefined {
  let ms: number;
  if (typeof v === 'string') {
    ms = Date.parse(v);
    if (Number.isNaN(ms)) return undefined;
  } else if (isNum(v)) {
    ms = v > 1e11 ? v : v * 1000;
  } else return undefined;
  return (Date.now() - ms) / 1000;
}

/** Evaluate a condition against flattened properties. */
export function matches(c: Condition, props: FlatProps): boolean {
  if ('all' in c) return c.all.every((x) => matches(x, props));
  if ('any' in c) return c.any.some((x) => matches(x, props));
  if ('not' in c) return !matches(c.not, props);
  const actual = props[c.prop];
  const present = actual !== undefined && actual !== null;
  const v = c.value;
  switch (c.op) {
    case 'exists': return present;
    case 'not_exists': return !present;
  }
  if (!present) return c.op === 'neq' || c.op === 'not_in' || c.op === 'not_contains';
  switch (c.op) {
    case 'eq': return looseEq(actual, v);
    case 'neq': return !looseEq(actual, v);
    case 'gt': case 'gte': case 'lt': case 'lte': {
      const o = compare(actual, v);
      if (o === undefined) return false;
      return c.op === 'gt' ? o > 0 : c.op === 'gte' ? o >= 0 : c.op === 'lt' ? o < 0 : o <= 0;
    }
    case 'in': return Array.isArray(v) && v.some((x) => looseEq(actual, x));
    case 'not_in': return !(Array.isArray(v) && v.some((x) => looseEq(actual, x)));
    case 'contains': return contains(actual, v);
    case 'not_contains': return !contains(actual, v);
    case 'starts_with': return typeof actual === 'string' && typeof v === 'string' && actual.startsWith(v);
    case 'ends_with': return typeof actual === 'string' && typeof v === 'string' && actual.endsWith(v);
    case 'matches': {
      if (typeof actual !== 'string' || typeof v !== 'string') return false;
      try { return new RegExp(v).test(actual); } catch { return false; }
    }
    case 'semver_eq': case 'semver_gt': case 'semver_gte': case 'semver_lt': case 'semver_lte': {
      const o = semverCmp(actual, v);
      if (o === undefined) return false;
      return c.op === 'semver_eq' ? o === 0 : c.op === 'semver_gt' ? o > 0 : c.op === 'semver_gte' ? o >= 0 : c.op === 'semver_lt' ? o < 0 : o <= 0;
    }
    case 'within_last': case 'older_than': {
      const age = ageSecs(actual), secs = asFloat(v);
      if (age === undefined || secs === undefined) return false;
      return c.op === 'within_last' ? age >= 0 && age <= secs : age > secs;
    }
  }
  return false;
}

export class LocalEvaluator {
  private readonly flags = new Map<string, RuleFlag>();
  private readonly audiences = new Map<string, RuleAudience>();
  private readonly managed: Set<string>;

  constructor(readonly bundle: RulesBundle) {
    for (const f of bundle.flags) this.flags.set(f.key, f);
    for (const a of bundle.audiences) this.audiences.set(a.key, a);
    this.managed = new Set(bundle.managed_audiences ?? []);
  }

  /** Dynamic-audience membership; managed audiences never match locally. */
  inAudience(key: string, subject: string, props: FlatProps): boolean {
    const a = this.audiences.get(key);
    if (!a || !a.enabled || !matches(a.condition, props)) return false;
    return inRollout(`audience:${key}`, a.salt, subject, a.percentage ?? 100);
  }

  /** True when a rule of the flag references a managed audience (prefer remote evaluation then). */
  dependsOnManaged(key: string): boolean {
    return (this.flags.get(key)?.rules ?? []).some((r) => (r.audiences ?? []).some((a) => this.managed.has(a)));
  }

  private one(f: RuleFlag, subject: string, props: FlatProps): Flag {
    const out: Flag = { key: f.key, value: f.default_value, value_type: f.value_type, reason: { kind: 'default' }, version: f.version };
    if (!f.enabled) return { ...out, reason: { kind: 'disabled' } };
    const ns = `flag:${f.key}`;
    for (const r of f.rules) {
      if (!matches(r.condition, props)) continue;
      if (!(r.audiences ?? []).every((a) => this.inAudience(a, subject, props))) continue;
      if (!inRollout(ns, `${f.salt}:${r.id}`, subject, r.percentage ?? 100)) continue;
      if (r.value !== undefined && r.value !== null) return { ...out, value: r.value, reason: { kind: 'rule', rule_id: r.id } };
      const variants = r.variants ?? [];
      const i = weightedPick(ns, `${f.salt}:${r.id}:variant`, subject, variants.map((v) => v.weight));
      if (i >= 0) {
        const v = variants[i]!;
        return { ...out, value: v.value, variant: v.key, reason: { kind: 'variant', rule_id: r.id, variant: v.key } };
      }
    }
    return out;
  }

  /** Resolve one flag; `undefined` when unknown. */
  evaluate(key: string, subject: string, props: FlatProps): Flag | undefined {
    const f = this.flags.get(key);
    if (!f) return undefined;
    const res = this.one(f, subject, props);
    for (const p of f.prerequisites ?? []) {
      const dep = this.flags.get(p.flag);
      const d = dep ? this.one(dep, subject, props) : undefined;
      const met = d && d.reason.kind !== 'disabled' && d.reason.kind !== 'prerequisite' && (p.value === undefined || p.value === null || JSON.stringify(p.value) === JSON.stringify(d.value));
      if (!met) {
        const { variant: _v, ...rest } = res;
        return { ...rest, value: f.default_value, reason: { kind: 'prerequisite', flag: p.flag } };
      }
    }
    return res;
  }

  /** Resolve all flags (or only `names`), sorted by key like the server. */
  evaluateAll(subject: string, props: FlatProps, names?: string[]): Flag[] {
    const keys = (names && names.length ? names.filter((n) => this.flags.has(n)) : [...this.flags.keys()]).sort();
    return keys.map((k) => this.evaluate(k, subject, props)!);
  }
}
