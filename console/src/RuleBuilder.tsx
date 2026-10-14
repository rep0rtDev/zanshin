import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { get } from './api';
import { JsonField } from './ui';

export type Cmp = { prop: string; op: string; value?: unknown };
export type Cond = { all: Cond[] } | { any: Cond[] } | { not: Cond } | Cmp;

export const OPS: [string, string, 'none' | 'one' | 'many'][] = [
  ['eq', 'equals', 'one'], ['neq', 'not equals', 'one'],
  ['gt', '>', 'one'], ['gte', '≥', 'one'], ['lt', '<', 'one'], ['lte', '≤', 'one'],
  ['in', 'is one of', 'many'], ['not_in', 'is not one of', 'many'],
  ['contains', 'contains', 'one'], ['not_contains', 'does not contain', 'one'],
  ['starts_with', 'starts with', 'one'], ['ends_with', 'ends with', 'one'], ['matches', 'matches regex', 'one'],
  ['exists', 'is set', 'none'], ['not_exists', 'is not set', 'none'],
  ['semver_eq', 'version =', 'one'], ['semver_gt', 'version >', 'one'], ['semver_gte', 'version ≥', 'one'], ['semver_lt', 'version <', 'one'], ['semver_lte', 'version ≤', 'one'],
];
const ARITY = Object.fromEntries(OPS.map(([k, , a]) => [k, a]));

export const PROP_SUGGESTIONS = [
  'identity.external_id', 'default.platform', 'default.country', 'default.language', 'default.app_version', 'default.device',
  'custom.level', 'custom.vip', 'custom.segment', 'computed.session_count', 'computed.first_seen_at', 'computed.last_seen_at',
];

const isGroup = (c: Cond): c is { all: Cond[] } | { any: Cond[] } => 'all' in c || 'any' in c;
const isNot = (c: Cond): c is { not: Cond } => 'not' in c;
const kids = (c: Cond): Cond[] => ('all' in c ? c.all : 'any' in c ? c.any : []);

function parseScalar(s: string): unknown {
  const t = s.trim();
  if (t === '') return '';
  if (t === 'true') return true;
  if (t === 'false') return false;
  if (t === 'null') return null;
  if (/^-?\d+(\.\d+)?$/.test(t)) return Number(t);
  if ((t.startsWith('"') && t.endsWith('"'))) return t.slice(1, -1);
  return t;
}
const showScalar = (v: unknown): string => (typeof v === 'string' ? (/^(true|false|null|-?\d+(\.\d+)?)$/.test(v) ? `"${v}"` : v) : v === undefined ? '' : JSON.stringify(v));

function CmpRow({ c, onChange, onRemove }: { c: Cmp; onChange: (c: Cmp) => void; onRemove: () => void }) {
  const arity = ARITY[c.op] ?? 'one';
  const [many, setMany] = useState(Array.isArray(c.value) ? (c.value as unknown[]).map(showScalar).join(', ') : '');
  useEffect(() => { if (arity !== 'many') return; setMany(Array.isArray(c.value) ? (c.value as unknown[]).map(showScalar).join(', ') : ''); }, [c.op]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <div className="rb-row">
      <input className="mono" list="rb-props" value={c.prop} placeholder="default.country" onChange={(e) => onChange({ ...c, prop: e.target.value })} style={{ flex: 2 }} />
      <select value={c.op} onChange={(e) => { const op = e.target.value; const a = ARITY[op]; onChange({ prop: c.prop, op, value: a === 'none' ? undefined : a === 'many' ? (Array.isArray(c.value) ? c.value : []) : Array.isArray(c.value) ? (c.value as unknown[])[0] : c.value }); }} style={{ flex: 1.3 }}>
        {OPS.map(([k, l]) => <option key={k} value={k}>{l}</option>)}
      </select>
      {arity === 'one' && <input className="mono" value={showScalar(c.value)} placeholder='value — 42, true, "text"' onChange={(e) => onChange({ ...c, value: parseScalar(e.target.value) })} style={{ flex: 2 }} />}
      {arity === 'many' && <input className="mono" value={many} placeholder="DE, FR, AT" onChange={(e) => { setMany(e.target.value); onChange({ ...c, value: e.target.value.split(',').map((s) => s.trim()).filter(Boolean).map(parseScalar) }); }} style={{ flex: 2 }} />}
      {arity === 'none' && <span style={{ flex: 2 }} />}
      <button type="button" className="btn ghost sm" onClick={onRemove} title="remove">×</button>
    </div>
  );
}

function Node({ c, onChange, onRemove, root }: { c: Cond; onChange: (c: Cond) => void; onRemove?: () => void; root?: boolean }) {
  if (isNot(c)) {
    return (
      <div className="rb-group not">
        <div className="rb-head">
          <span className="badge bad">NOT</span>
          <span className="spacer" />
          <button type="button" className="btn ghost sm" onClick={() => onChange(c.not)}>unwrap</button>
          {onRemove && <button type="button" className="btn ghost sm" onClick={onRemove}>×</button>}
        </div>
        <Node c={c.not} onChange={(n) => onChange({ not: n })} />
      </div>
    );
  }
  if (isGroup(c)) {
    const mode: 'all' | 'any' = 'all' in c ? 'all' : 'any';
    const list = kids(c);
    const set = (l: Cond[]) => onChange(mode === 'all' ? { all: l } : { any: l });
    return (
      <div className={'rb-group' + (root ? ' root' : '')}>
        <div className="rb-head">
          <select value={mode} onChange={(e) => onChange(e.target.value === 'all' ? { all: list } : { any: list })} className="rb-mode">
            <option value="all">ALL of</option>
            <option value="any">ANY of</option>
          </select>
          {list.length === 0 && <span className="hint">no conditions — matches everyone</span>}
          <span className="spacer" />
          <button type="button" className="btn ghost sm" onClick={() => set([...list, { prop: '', op: 'eq', value: '' }])}>+ condition</button>
          <button type="button" className="btn ghost sm" onClick={() => set([...list, { all: [] }])}>+ group</button>
          <button type="button" className="btn ghost sm" onClick={() => set([...list, { not: { prop: '', op: 'eq', value: '' } }])}>+ not</button>
          {onRemove && <button type="button" className="btn ghost sm" onClick={onRemove}>×</button>}
        </div>
        {list.map((k, i) => (
          <Node key={i} c={k} onChange={(n) => set(list.map((x, j) => (j === i ? n : x)))} onRemove={() => set(list.filter((_, j) => j !== i))} />
        ))}
      </div>
    );
  }
  return <CmpRow c={c} onChange={onChange} onRemove={onRemove ?? (() => {})} />;
}

/** Visual editor for a targeting condition. `optional` renders a "match everyone" state for `null`. */
export function ConditionEditor({ value, onChange, optional }: { value: Cond | null | undefined; onChange: (c: Cond | null) => void; optional?: boolean }) {
  const [raw, setRaw] = useState(false);
  const empty = value === null || value === undefined;
  return (
    <div className="rb">
      <datalist id="rb-props">{PROP_SUGGESTIONS.map((p) => <option key={p} value={p} />)}</datalist>
      <div className="rb-tools">
        {optional && !empty && <button type="button" className="btn ghost sm" onClick={() => onChange(null)}>clear (everyone)</button>}
        <button type="button" className="btn ghost sm" onClick={() => setRaw(!raw)}>{raw ? 'visual' : 'JSON'}</button>
      </div>
      {raw ? (
        <JsonField value={empty ? undefined : value} onChange={(v) => onChange((v as Cond) ?? null)} rows={6} placeholder='{"all": []}' />
      ) : empty ? (
        <div className="rb-empty">
          <span className="muted">Everyone{optional ? '' : ' (no condition)'}</span>
          <button type="button" className="btn ghost sm" onClick={() => onChange({ all: [{ prop: '', op: 'eq', value: '' }] })}>+ add condition</button>
        </div>
      ) : (
        <Node c={value as Cond} onChange={onChange} root />
      )}
    </div>
  );
}

/** Multi-select of audience keys fetched from the server. */
export function AudiencePicker({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  value = value ?? [];
  const [opts, setOpts] = useState<{ key: string; name: string }[] | null>(null);
  useEffect(() => { get<{ key: string; name: string }[]>('/audiences').then(setOpts, () => setOpts([])); }, []);
  const toggle = (k: string) => onChange(value.includes(k) ? value.filter((x) => x !== k) : [...value, k]);
  if (!opts) return <span className="hint">loading audiences…</span>;
  if (!opts.length) return <span className="hint">no audiences defined yet</span>;
  return (
    <div className="chips">
      {opts.map((a) => (
        <button type="button" key={a.key} className={'chip' + (value.includes(a.key) ? ' on' : '')} onClick={() => toggle(a.key)} title={a.name}>{a.key}</button>
      ))}
      {value.filter((v) => !opts.some((o) => o.key === v)).map((v) => <span key={v} className="chip missing" title="unknown audience">{v}</span>)}
    </div>
  );
}

export type ValueType = 'bool' | 'string' | 'number' | 'json';
/** Input for a flag value according to its declared type. */
export function TypedValue({ type, value, onChange, compact }: { type: ValueType; value: unknown; onChange: (v: unknown) => void; compact?: boolean }) {
  if (type === 'bool') return (
    <select value={String(!!value)} onChange={(e) => onChange(e.target.value === 'true')}>
      <option value="true">true</option><option value="false">false</option>
    </select>
  );
  if (type === 'number') return <input type="number" step="any" value={typeof value === 'number' ? value : ''} onChange={(e) => onChange(e.target.value === '' ? 0 : Number(e.target.value))} />;
  if (type === 'string') return <input value={typeof value === 'string' ? value : ''} onChange={(e) => onChange(e.target.value)} />;
  return <JsonField value={value} onChange={(v) => onChange(v ?? null)} rows={compact ? 1 : 3} />;
}

export type Variant = { key: string; value: unknown; weight: number; name?: string };
export function VariantsEditor({ type, value, onChange, withName }: { type: ValueType; value: Variant[]; onChange: (v: Variant[]) => void; withName?: boolean }) {
  value = value ?? [];
  const total = value.reduce((a, v) => a + (Number(v.weight) || 0), 0);
  const set = (i: number, patch: Partial<Variant>) => onChange(value.map((v, j) => (j === i ? { ...v, ...patch } : v)));
  return (
    <div className="variants">
      {value.map((v, i) => (
        <div key={i} className="rb-row">
          <input className="mono" value={v.key} placeholder="key" onChange={(e) => set(i, { key: e.target.value })} style={{ flex: 1 }} />
          {withName && <input value={v.name ?? ''} placeholder="name" onChange={(e) => set(i, { name: e.target.value })} style={{ flex: 1.2 }} />}
          <div style={{ flex: 1.5 }}><TypedValue type={type} value={v.value} onChange={(val) => set(i, { value: val })} compact /></div>
          <input type="number" min={0} value={v.weight} onChange={(e) => set(i, { weight: Number(e.target.value) })} style={{ flex: 0.6 }} title="weight" />
          <span className="hint" style={{ width: 44, textAlign: 'right' }}>{total ? Math.round(((Number(v.weight) || 0) / total) * 100) : 0}%</span>
          <button type="button" className="btn ghost sm" onClick={() => onChange(value.filter((_, j) => j !== i))}>×</button>
        </div>
      ))}
      <button type="button" className="btn ghost sm" onClick={() => onChange([...value, { key: `v${value.length + 1}`, value: type === 'bool' ? true : type === 'number' ? 0 : type === 'string' ? '' : {}, weight: 1 }])}>+ variant</button>
    </div>
  );
}

export type Rule = { id: string; name: string; condition: Cond; audiences: string[]; percentage: number; value?: unknown; variants: Variant[] };
const newId = () => 'r' + Math.random().toString(36).slice(2, 8);

/** Ordered list of flag rules; first match wins. */
export function RulesEditor({ type, value, onChange }: { type: ValueType; value: Rule[]; onChange: (r: Rule[]) => void }) {
  const set = (i: number, patch: Partial<Rule>) => onChange(value.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const move = (i: number, d: number) => { const j = i + d; if (j < 0 || j >= value.length) return; const l = [...value]; [l[i], l[j]] = [l[j], l[i]]; onChange(l); };
  return (
    <div className="rules">
      {value.length === 0 && <div className="muted empty">No rules — every identity gets the default value.</div>}
      {value.map((r, i) => {
        const variants = r.variants ?? [];
        const audiences = r.audiences ?? [];
        const weighted = variants.length > 0;
        return (
          <div key={r.id || i} className="rule card">
            <div className="rb-head">
              <span className="badge info">#{i + 1}</span>
              <input value={r.name} placeholder="rule name" onChange={(e) => set(i, { name: e.target.value })} style={{ flex: 1 }} />
              <button type="button" className="btn ghost sm" disabled={i === 0} onClick={() => move(i, -1)}>↑</button>
              <button type="button" className="btn ghost sm" disabled={i === value.length - 1} onClick={() => move(i, 1)}>↓</button>
              <button type="button" className="btn ghost sm danger" onClick={() => onChange(value.filter((_, j) => j !== i))}>remove</button>
            </div>
            <Label>Condition</Label>
            <ConditionEditor value={r.condition ?? { all: [] }} onChange={(c) => set(i, { condition: c ?? { all: [] } })} />
            <Label>Audiences <span className="hint">(identity must belong to all selected)</span></Label>
            <AudiencePicker value={audiences} onChange={(audiences) => set(i, { audiences })} />
            <div className="row" style={{ marginTop: 10 }}>
              <label className="field" style={{ flex: 1 }}>
                <span className="label">Rollout %</span>
                <div className="inline"><input type="range" min={0} max={100} value={r.percentage} onChange={(e) => set(i, { percentage: Number(e.target.value) })} /><input type="number" min={0} max={100} value={r.percentage} onChange={(e) => set(i, { percentage: Number(e.target.value) })} style={{ width: 72 }} /></div>
              </label>
              <label className="field" style={{ flex: 1 }}>
                <span className="label">Serve</span>
                <select value={weighted ? 'variants' : 'value'} onChange={(e) => (e.target.value === 'variants' ? set(i, { value: undefined, variants: [{ key: 'a', value: r.value ?? true, weight: 1 }, { key: 'b', value: type === 'bool' ? false : r.value ?? '', weight: 1 }] }) : set(i, { variants: [], value: variants[0]?.value ?? true }))}>
                  <option value="value">a single value</option>
                  <option value="variants">weighted variants</option>
                </select>
              </label>
            </div>
            {weighted ? <VariantsEditor type={type} value={variants} onChange={(variants) => set(i, { variants })} /> : (
              <label className="field"><span className="label">Value</span><TypedValue type={type} value={r.value} onChange={(value) => set(i, { value })} /></label>
            )}
          </div>
        );
      })}
      <button type="button" className="btn ghost" onClick={() => onChange([...value, { id: newId(), name: '', condition: { all: [] }, audiences: [], percentage: 100, value: type === 'bool' ? true : type === 'number' ? 0 : type === 'string' ? '' : {}, variants: [] }])}>+ add rule</button>
    </div>
  );
}

const Label = ({ children }: { children: ReactNode }) => <div className="label" style={{ margin: '10px 0 4px' }}>{children}</div>;
