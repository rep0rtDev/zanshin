import { useState } from 'react';
import type { ReactNode } from 'react';
import { get, post, put, useLoad, del } from '../api';
import { EntityPage, Text, Tags } from '../crud';
import { RulesEditor, TypedValue } from '../RuleBuilder';
import type { Rule } from '../RuleBuilder';
import { Badge, Confirm, ErrorBox, Field, JsonField, Table, Toggle, fmtDate } from '../ui';
import { t } from '../i18n';

type Prerequisite = { flag: string; value?: unknown };
type Schedule = { id: string; at: string; patch: unknown; status: string; created_by: string; note: string; error: string; applied_at?: string | null };
type Flag = {
  prerequisites?: Prerequisite[];
  key: string; name: string; description: string; value_type: 'bool' | 'string' | 'number' | 'json'; default_value: unknown;
  enabled: boolean; rules: Rule[]; tags: string[]; version: number; updated_at: string;
};
type Override = { identity_id: string; external_id: string; value: unknown };

function Overrides({ flag, reload }: { flag: Flag; reload: () => void }) {
  const q = useLoad(() => get<Override[]>(`/flags/${encodeURIComponent(flag.key)}/overrides`), [flag.key]);
  const [ext, setExt] = useState('');
  const [val, setVal] = useState<unknown>(undefined);
  const [err, setErr] = useState<Error | null>(null);
  const add = async () => {
    try {
      const id = await get<{ id: string }>(`/identities/by-external-id/${encodeURIComponent(ext)}`);
      await put(`/flags/${encodeURIComponent(flag.key)}/overrides/${id.id}`, { value: val });
      setExt(''); q.reload(); setErr(null);
    } catch (e) { setErr(e as Error); }
  };
  return (
    <div className="card">
      <strong>{t('Per-identity overrides')}</strong>
      <ErrorBox error={err ?? q.error} />
      <div className="row" style={{ marginTop: 10 }}>
        <Field label={t('External id')}><input value={ext} onChange={(e) => setExt(e.target.value)} placeholder="player-123" /></Field>
        <Field label={t('Value (JSON)')}><JsonField value={undefined} onChange={setVal} rows={1} placeholder="true" /></Field>
        <div className="field fixed"><span className="label">&nbsp;</span><button className="btn" disabled={!ext || val === undefined} onClick={add}>{t('Set')}</button></div>
      </div>
      <Table<Override>
        rows={q.data}
        rowKey={(o) => o.identity_id}
        empty={t('No overrides')}
        cols={[
          { h: 'Identity', c: (o) => <span className="mono">{o.external_id}</span> },
          { h: 'Value', c: (o) => <code>{JSON.stringify(o.value)}</code> },
          { h: '', w: '110px', c: (o) => <Confirm danger onConfirm={() => del(`/flags/${encodeURIComponent(flag.key)}/overrides/${o.identity_id}`).then(() => { q.reload(); reload(); }, setErr)}>{t('Remove')}</Confirm> },
        ]}
      />
    </div>
  );
}

type ExplainRule = { id: string; name: string; condition_matches: boolean; audiences_match: boolean; in_rollout: boolean; value: unknown };
type ExplainOut = { enabled: boolean; identity_override: unknown; experiment: { experiment: string; variant: string; value: unknown } | null; live_event: { live_event: string; value: unknown } | null; rules: ExplainRule[]; default_value: unknown; audiences: string[]; now: string; result: { value: unknown; reason: { kind: string } } };

type Size = { selector: string; bytes: number };
type FlagDiff = { a: { selector: string; bytes: number; value: unknown }; b: { selector: string; bytes: number; value: unknown }; changes: { path: string; from?: unknown; to?: unknown }[] };

function Diff({ flag }: { flag: Flag }) {
  const sizes = useLoad(() => get<Size[]>(`/flags/${flag.key}/sizes`), [flag.key, flag.version]);
  const [a, setA] = useState('default');
  const [b, setB] = useState('default');
  const [out, setOut] = useState<FlagDiff | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const opts = sizes.data ?? [];
  const run = () => get<FlagDiff>(`/flags/${flag.key}/diff?a=${encodeURIComponent(a)}&b=${encodeURIComponent(b)}`).then((r) => { setOut(r); setErr(null); }, setErr);
  return (
    <div className="card">
      <strong>{t('Payload sizes')}</strong>
      <div className="hint">{t('Bytes the client receives for each value (default, rule values and variants).')}</div>
      <Table rows={opts} rowKey={(r) => r.selector} cols={[{ h: 'Value', c: (r) => <code>{r.selector}</code> }, { h: 'Bytes', c: (r) => r.bytes.toLocaleString() }]} />
      <strong style={{ display: 'block', marginTop: 12 }}>{t('Compare two values')}</strong>
      <div className="row">
        <Field label="A"><select value={a} onChange={(e) => setA(e.target.value)}>{opts.map((o) => <option key={o.selector} value={o.selector}>{o.selector}</option>)}</select></Field>
        <Field label="B"><select value={b} onChange={(e) => setB(e.target.value)}>{opts.map((o) => <option key={o.selector} value={o.selector}>{o.selector}</option>)}</select></Field>
        <button className="btn" onClick={run}>{t('Diff')}</button>
      </div>
      <ErrorBox error={err} />
      {out && (out.changes.length === 0 ? <div className="hint">No differences ({out.a.bytes} vs {out.b.bytes} bytes).</div> : (
        <Table rows={out.changes} rowKey={(c) => c.path} cols={[
          { h: 'Path', c: (c) => <code>{c.path || '(root)'}</code> },
          { h: out.a.selector, c: (c) => <code>{c.from === undefined ? '—' : JSON.stringify(c.from)}</code> },
          { h: out.b.selector, c: (c) => <code>{c.to === undefined ? '—' : JSON.stringify(c.to)}</code> },
        ]} />
      ))}
    </div>
  );
}

function Explain({ flag }: { flag: Flag }) {
  const [ext, setExt] = useState('');
  const [out, setOut] = useState<ExplainOut | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const run = async () => {
    try {
      const id = await get<{ id: string }>(`/identities/by-external-id/${encodeURIComponent(ext)}`);
      setOut(await get<ExplainOut>(`/flags/${encodeURIComponent(flag.key)}/explain/${id.id}`)); setErr(null);
    } catch (e) { setErr(e as Error); }
  };
  const J = (v: unknown) => <code>{JSON.stringify(v)}</code>;
  const layer = (name: string, hit: boolean, body: ReactNode) => <li key={name} style={{ opacity: hit ? 1 : 0.55 }}><Badge tone={hit ? 'ok' : undefined}>{name}</Badge> {body}</li>;
  return (
    <div className="card">
      <strong>{t('Explain for a player')}</strong>
      <div className="hint">{t('Override chain: identity override → experiment → live event → rules → default.')}</div>
      <ErrorBox error={err} />
      <div className="row" style={{ marginTop: 10 }}>
        <Field label={t('External id')}><input value={ext} onChange={(e) => setExt(e.target.value)} placeholder="player-123" /></Field>
        <div className="field fixed"><span className="label">&nbsp;</span><button className="btn ghost" disabled={!ext} onClick={run}>{t('Explain')}</button></div>
      </div>
      {out && (
        <>
          <div style={{ marginTop: 10 }}>Result: {J(out.result.value)} <Badge tone="info">{out.result.reason.kind}</Badge> <span className="hint">clock {fmtDate(out.now)} · audiences: {out.audiences.join(', ') || '—'}</span></div>
          <ol className="chain">
            {layer('identity override', out.identity_override != null, out.identity_override != null ? J(out.identity_override) : <span className="muted">none</span>)}
            {layer('experiment', !!out.experiment, out.experiment ? <>{out.experiment.experiment} / {out.experiment.variant} → {J(out.experiment.value)}</> : <span className="muted">{t('not enrolled')}</span>)}
            {layer('live event', !!out.live_event, out.live_event ? <>{out.live_event.live_event} → {J(out.live_event.value)}</> : <span className="muted">{t('no active event overrides this flag')}</span>)}
            {out.rules.map((r) => layer(`rule ${r.name || r.id}`, r.condition_matches && r.audiences_match && r.in_rollout, <>{r.condition_matches ? 'condition ✓' : 'condition ✗'} · {r.audiences_match ? 'audiences ✓' : 'audiences ✗'} · {r.in_rollout ? 'rollout ✓' : 'rollout ✗'} → {J(r.value)}</>))}
            {layer('default', true, J(out.default_value))}
          </ol>
        </>
      )}
    </div>
  );
}

function DryRun({ flag }: { flag: Flag }) {
  const [props, setProps] = useState<unknown>({ default: { country: 'DE', platform: 'ios' }, custom: {} });
  const [subject, setSubject] = useState('dry-run');
  const [out, setOut] = useState<unknown>(null);
  const [err, setErr] = useState<Error | null>(null);
  const run = () => post(`/flags/${encodeURIComponent(flag.key)}/evaluate`, { subject, properties: props }).then((r) => { setOut(r); setErr(null); }, setErr);
  return (
    <div className="card">
      <strong>{t('Dry run')}</strong>
      <div className="hint" style={{ marginBottom: 8 }}>{t('Evaluate this flag against hypothetical properties; the subject string drives percentage bucketing.')}</div>
      <ErrorBox error={err} />
      <Field label={t('Subject')}><input value={subject} onChange={(e) => setSubject(e.target.value)} /></Field>
      <Field label={t('Properties')}><JsonField value={props} onChange={setProps} rows={5} /></Field>
      <button className="btn ghost" onClick={run}>{t('Evaluate')}</button>
      {out !== null && <pre className="code" style={{ marginTop: 10 }}>{JSON.stringify(out, null, 2)}</pre>}
    </div>
  );
}

function PrerequisitesEditor({ self, value, onChange }: { self: string; value: Prerequisite[]; onChange: (v: Prerequisite[]) => void }) {
  const q = useLoad(() => get<{ items: Flag[] }>('/flags'), []);
  const flags = (q.data?.items ?? []).filter((f) => f.key !== self);
  const upd = (i: number, p: Partial<Prerequisite>) => onChange(value.map((x, j) => (j === i ? { ...x, ...p } : x)));
  return (
    <div className="stack" style={{ gap: 6 }}>
      {value.map((p, i) => (
        <div className="row" key={i} style={{ alignItems: 'end' }}>
          <label className="field"><span className="label">{t('Flag')}</span>
            <select value={p.flag} onChange={(e) => upd(i, { flag: e.target.value })}>
              <option value="">— pick —</option>
              {flags.map((f) => <option key={f.key} value={f.key}>{f.key}</option>)}
            </select>
          </label>
          <Field label={t('Required value (JSON, empty = just enabled)')}><input className="mono" value={p.value === undefined ? '' : JSON.stringify(p.value)} onChange={(e) => { const v = e.target.value; try { upd(i, { value: v.trim() ? JSON.parse(v) : undefined }); } catch { /* keep typing */ } }} /></Field>
          <button type="button" className="btn ghost sm" onClick={() => onChange(value.filter((_, j) => j !== i))}>remove</button>
        </div>
      ))}
      <div><button type="button" className="btn ghost sm" onClick={() => onChange([...value, { flag: '' }])}>+ add prerequisite</button></div>
    </div>
  );
}

function SchedulePanel({ flag, reload }: { flag: Flag; reload: () => void }) {
  const q = useLoad(() => get<{ items: Schedule[] }>(`/flags/${encodeURIComponent(flag.key)}/schedules`), [flag.key, flag.version]);
  const [at, setAt] = useState('');
  const [patch, setPatch] = useState('{\n  "enabled": false\n}');
  const [note, setNote] = useState('');
  const [err, setErr] = useState<Error | null>(null);
  const submit = async () => {
    try {
      await post(`/flags/${encodeURIComponent(flag.key)}/schedules`, { at: new Date(at).toISOString(), patch: JSON.parse(patch), note });
      setErr(null); setAt(''); setNote(''); q.reload();
    } catch (e) { setErr(e as Error); }
  };
  return (
    <div className="stack">
      <div className="card">
        <strong>{t('Kill switch')}</strong> <span className="hint">disable the flag for everyone right now; clients fall back to the default</span>
        <div style={{ marginTop: 8 }}>
          {flag.enabled
            ? <Confirm danger onConfirm={() => post(`/flags/${encodeURIComponent(flag.key)}/kill`).then(reload, setErr)}>{t('Kill flag')}</Confirm>
            : <button className="btn" onClick={() => post(`/flags/${encodeURIComponent(flag.key)}/restore`).then(reload, setErr)}>{t('Restore (enable)')}</button>}
        </div>
      </div>
      <div className="card">
        <strong>{t('Scheduled changes')}</strong> <span className="hint">{t('a patch applied at the given time (checked every minute)')}</span>
        <ErrorBox error={err ?? q.error} />
        <Table<Schedule> rows={q.data?.items ?? []} rowKey={(s) => s.id} empty={t('Nothing scheduled.')}
          cols={[
            { h: 'When', c: (s) => fmtDate(s.at) },
            { h: 'Patch', c: (s) => <code>{JSON.stringify(s.patch)}</code> },
            { h: 'Status', c: (s) => <Badge tone={s.status === 'applied' ? 'ok' : s.status === 'failed' ? 'bad' : s.status === 'pending' ? 'info' : undefined}>{s.status}</Badge> },
            { h: 'Note', c: (s) => <>{s.note}{s.error && <div className="hint">{s.error}</div>}</> },
            { h: 'By', c: (s) => s.created_by },
            { h: '', c: (s) => s.status === 'pending' ? <button className="btn ghost sm" onClick={() => del(`/flags/${encodeURIComponent(flag.key)}/schedules/${s.id}`).then(() => q.reload(), setErr)}>cancel</button> : null },
          ]} />
        <div className="row" style={{ alignItems: 'end', marginTop: 8 }}>
          <label className="field"><span className="label">{t('At (local time)')}</span><input type="datetime-local" value={at} onChange={(e) => setAt(e.target.value)} /></label>
          <Field label={t('Patch (flag fields)')}><textarea className="mono" rows={3} value={patch} onChange={(e) => setPatch(e.target.value)} /></Field>
          <Text label={t('Note')} value={note} onChange={setNote} />
          <button className="btn" disabled={!at} onClick={submit}>{t('Schedule')}</button>
        </div>
      </div>
    </div>
  );
}

type Stats = { total: number; days: number; last_day?: string | null; by_variant: Record<string, number>; series: { day: string; variant: string; count: number }[] };
function StatsPanel({ flag }: { flag: Flag; reload: () => void }) {
  const [days, setDays] = useState(30);
  const q = useLoad(() => get<Stats>(`/flags/${encodeURIComponent(flag.key)}/stats?days=${days}`), [flag.key, days]);
  const s = q.data;
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}>
        <strong>{t('Evaluations')}</strong>
        <select value={days} onChange={(e) => setDays(Number(e.target.value))}>{[7, 30, 90].map((d) => <option key={d} value={d}>last {d} days</option>)}</select>
        {s && <span className="hint">{s.total.toLocaleString()} evaluations · last {s.last_day ?? '—'}{s.total === 0 ? ' · ' : ''}{s.total === 0 && <Badge tone="warn">{t('no traffic — candidate for cleanup')}</Badge>}</span>}
      </div>
      <ErrorBox error={q.error} />
      {s && (
        <Table rows={Object.entries(s.by_variant)} rowKey={([v]) => v} empty={t('No evaluations in this window.')} cols={[
          { h: 'Served variant', c: ([v]) => v ? <code>{v}</code> : <span className="hint">(value / default)</span> },
          { h: 'Evaluations', c: ([, n]) => n.toLocaleString() },
          { h: 'Share', c: ([, n]) => s.total ? ((n / s.total) * 100).toFixed(1) + '%' : '—' },
        ]} />
      )}
    </div>
  );
}

export default function Flags() {
  return (
    <EntityPage<Flag>
      title={t('Feature flags')}
      path="/flags"
      blurb={t('Remote-configurable values with targeting rules, percentage rollouts and weighted variants. Changes reach connected clients instantly.')}
      cols={[
        { h: 'Key', c: (f) => <><strong>{f.key}</strong><div className="hint">{f.name}</div></> },
        { h: 'Type', c: (f) => <Badge tone="info">{f.value_type}</Badge> },
        { h: 'Default', c: (f) => <code>{JSON.stringify(f.default_value)}</code> },
        { h: 'Rules', c: (f) => (f.rules ?? []).length },
        { h: 'Status', c: (f) => f.enabled ? <Badge tone="ok">enabled</Badge> : <Badge>disabled</Badge> },
        { h: 'Updated', c: (f) => fmtDate(f.updated_at) },
      ]}
      blank={() => ({ key: '', name: '', description: '', value_type: 'bool', default_value: false, enabled: true, rules: [], tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label={t('Key')} value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono hint={t('immutable, used by SDKs')} />
            <Text label={t('Name')} value={d.name} onChange={(name) => set({ name })} />
          </div>
          <Text label={t('Description')} value={d.description} onChange={(description) => set({ description })} />
          <div className="row">
            <label className="field">
              <span className="label">{t('Value type')}</span>
              <select value={d.value_type} onChange={(e) => { const t = e.target.value as Flag['value_type']; set({ value_type: t, default_value: t === 'bool' ? false : t === 'number' ? 0 : t === 'string' ? '' : {} }); }}>
                <option value="bool">bool</option><option value="string">string</option><option value="number">number</option><option value="json">json</option>
              </select>
            </label>
            <Field label={t('Default value')}><TypedValue type={d.value_type ?? 'bool'} value={d.default_value} onChange={(v) => set({ default_value: v })} compact /></Field>
            <div className="field fixed"><span className="label">{t('Enabled')}</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <div className="label" style={{ margin: '4px 0 6px' }}>{t('Rules') + ' '}<span className="hint">— evaluated top to bottom, first match wins</span></div>
          <RulesEditor type={d.value_type ?? 'bool'} value={d.rules ?? []} onChange={(rules) => set({ rules })} />
          <Field label={t('Prerequisites')} hint="rules apply only while these flags are enabled and (optionally) equal the given value; otherwise the default is served">
            <PrerequisitesEditor self={d.key ?? ''} value={d.prerequisites ?? []} onChange={(prerequisites) => set({ prerequisites })} />
          </Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      wizard={[
        { label: 'Basics', render: (d, set) => (<><div className="row"><Text label={t('Key')} value={d.key} onChange={(key) => set({ key })} mono hint={t('immutable, used by SDKs')} /><Text label={t('Name')} value={d.name} onChange={(name) => set({ name })} /></div><Text label={t('Description')} value={d.description} onChange={(description) => set({ description })} /><Tags value={d.tags} onChange={(tags) => set({ tags })} /></>) },
        { label: 'Value', render: (d, set) => (<div className="row"><label className="field"><span className="label">{t('Value type')}</span><select value={d.value_type} onChange={(e) => { const t = e.target.value as Flag['value_type']; set({ value_type: t, default_value: t === 'bool' ? false : t === 'number' ? 0 : t === 'string' ? '' : {}, rules: [] }); }}><option value="bool">bool</option><option value="string">string</option><option value="number">number</option><option value="json">json</option></select></label><Field label={t('Default value')}><TypedValue type={d.value_type ?? 'bool'} value={d.default_value} onChange={(v) => set({ default_value: v })} compact /></Field><div className="field fixed"><span className="label">{t('Enabled')}</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div></div>) },
        { label: 'Targeting', render: (d, set) => (<><div className="label" style={{ margin: '4px 0 6px' }}>{t('Rules') + ' '}<span className="hint">— evaluated top to bottom, first match wins; leave empty to serve the default to everyone</span></div><RulesEditor type={d.value_type ?? 'bool'} value={d.rules ?? []} onChange={(rules) => set({ rules })} /></>) },
      ]}
      tabs={[
        { id: 'overrides', label: 'Overrides', render: (f, reload) => <Overrides flag={f} reload={reload} /> },
        { id: 'explain', label: 'Explain', render: (f) => <Explain flag={f} /> },
        { id: 'diff', label: 'Diff & size', render: (f) => <Diff flag={f} /> },
        { id: 'dryrun', label: 'Dry run', render: (f) => <DryRun flag={f} /> },
        { id: 'schedule', label: 'Schedule', render: (f, reload) => <SchedulePanel flag={f} reload={reload} /> },
        { id: 'stats', label: 'Stats', render: (f, reload) => <StatsPanel flag={f} reload={reload} /> },
      ]}
    />
  );
}
