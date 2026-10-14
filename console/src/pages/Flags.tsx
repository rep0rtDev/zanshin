import { useState } from 'react';
import type { ReactNode } from 'react';
import { get, post, put, useLoad, del } from '../api';
import { EntityPage, Text, Tags } from '../crud';
import { RulesEditor, TypedValue } from '../RuleBuilder';
import type { Rule } from '../RuleBuilder';
import { Badge, Confirm, ErrorBox, Field, JsonField, Table, Toggle, fmtDate } from '../ui';

type Flag = {
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
      <strong>Per-identity overrides</strong>
      <ErrorBox error={err ?? q.error} />
      <div className="row" style={{ marginTop: 10 }}>
        <Field label="External id"><input value={ext} onChange={(e) => setExt(e.target.value)} placeholder="player-123" /></Field>
        <Field label="Value (JSON)"><JsonField value={undefined} onChange={setVal} rows={1} placeholder="true" /></Field>
        <div className="field fixed"><span className="label">&nbsp;</span><button className="btn" disabled={!ext || val === undefined} onClick={add}>Set</button></div>
      </div>
      <Table<Override>
        rows={q.data}
        rowKey={(o) => o.identity_id}
        empty="No overrides"
        cols={[
          { h: 'Identity', c: (o) => <span className="mono">{o.external_id}</span> },
          { h: 'Value', c: (o) => <code>{JSON.stringify(o.value)}</code> },
          { h: '', w: '110px', c: (o) => <Confirm danger onConfirm={() => del(`/flags/${encodeURIComponent(flag.key)}/overrides/${o.identity_id}`).then(() => { q.reload(); reload(); }, setErr)}>Remove</Confirm> },
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
      <strong>Payload sizes</strong>
      <div className="hint">Bytes the client receives for each value (default, rule values and variants).</div>
      <Table rows={opts} rowKey={(r) => r.selector} cols={[{ h: 'Value', c: (r) => <code>{r.selector}</code> }, { h: 'Bytes', c: (r) => r.bytes.toLocaleString() }]} />
      <strong style={{ display: 'block', marginTop: 12 }}>Compare two values</strong>
      <div className="row">
        <Field label="A"><select value={a} onChange={(e) => setA(e.target.value)}>{opts.map((o) => <option key={o.selector} value={o.selector}>{o.selector}</option>)}</select></Field>
        <Field label="B"><select value={b} onChange={(e) => setB(e.target.value)}>{opts.map((o) => <option key={o.selector} value={o.selector}>{o.selector}</option>)}</select></Field>
        <button className="btn" onClick={run}>Diff</button>
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
      <strong>Explain for a player</strong>
      <div className="hint">Override chain: identity override → experiment → live event → rules → default.</div>
      <ErrorBox error={err} />
      <div className="row" style={{ marginTop: 10 }}>
        <Field label="External id"><input value={ext} onChange={(e) => setExt(e.target.value)} placeholder="player-123" /></Field>
        <div className="field fixed"><span className="label">&nbsp;</span><button className="btn ghost" disabled={!ext} onClick={run}>Explain</button></div>
      </div>
      {out && (
        <>
          <div style={{ marginTop: 10 }}>Result: {J(out.result.value)} <Badge tone="info">{out.result.reason.kind}</Badge> <span className="hint">clock {fmtDate(out.now)} · audiences: {out.audiences.join(', ') || '—'}</span></div>
          <ol className="chain">
            {layer('identity override', out.identity_override != null, out.identity_override != null ? J(out.identity_override) : <span className="muted">none</span>)}
            {layer('experiment', !!out.experiment, out.experiment ? <>{out.experiment.experiment} / {out.experiment.variant} → {J(out.experiment.value)}</> : <span className="muted">not enrolled</span>)}
            {layer('live event', !!out.live_event, out.live_event ? <>{out.live_event.live_event} → {J(out.live_event.value)}</> : <span className="muted">no active event overrides this flag</span>)}
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
      <strong>Dry run</strong>
      <div className="hint" style={{ marginBottom: 8 }}>Evaluate this flag against hypothetical properties; the subject string drives percentage bucketing.</div>
      <ErrorBox error={err} />
      <Field label="Subject"><input value={subject} onChange={(e) => setSubject(e.target.value)} /></Field>
      <Field label="Properties"><JsonField value={props} onChange={setProps} rows={5} /></Field>
      <button className="btn ghost" onClick={run}>Evaluate</button>
      {out !== null && <pre className="code" style={{ marginTop: 10 }}>{JSON.stringify(out, null, 2)}</pre>}
    </div>
  );
}

export default function Flags() {
  return (
    <EntityPage<Flag>
      title="Feature flags"
      path="/flags"
      blurb="Remote-configurable values with targeting rules, percentage rollouts and weighted variants. Changes reach connected clients instantly."
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
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono hint="immutable, used by SDKs" />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} />
          </div>
          <Text label="Description" value={d.description} onChange={(description) => set({ description })} />
          <div className="row">
            <label className="field">
              <span className="label">Value type</span>
              <select value={d.value_type} onChange={(e) => { const t = e.target.value as Flag['value_type']; set({ value_type: t, default_value: t === 'bool' ? false : t === 'number' ? 0 : t === 'string' ? '' : {} }); }}>
                <option value="bool">bool</option><option value="string">string</option><option value="number">number</option><option value="json">json</option>
              </select>
            </label>
            <Field label="Default value"><TypedValue type={d.value_type ?? 'bool'} value={d.default_value} onChange={(v) => set({ default_value: v })} compact /></Field>
            <div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <div className="label" style={{ margin: '4px 0 6px' }}>Rules <span className="hint">— evaluated top to bottom, first match wins</span></div>
          <RulesEditor type={d.value_type ?? 'bool'} value={d.rules ?? []} onChange={(rules) => set({ rules })} />
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      wizard={[
        { label: 'Basics', render: (d, set) => (<><div className="row"><Text label="Key" value={d.key} onChange={(key) => set({ key })} mono hint="immutable, used by SDKs" /><Text label="Name" value={d.name} onChange={(name) => set({ name })} /></div><Text label="Description" value={d.description} onChange={(description) => set({ description })} /><Tags value={d.tags} onChange={(tags) => set({ tags })} /></>) },
        { label: 'Value', render: (d, set) => (<div className="row"><label className="field"><span className="label">Value type</span><select value={d.value_type} onChange={(e) => { const t = e.target.value as Flag['value_type']; set({ value_type: t, default_value: t === 'bool' ? false : t === 'number' ? 0 : t === 'string' ? '' : {}, rules: [] }); }}><option value="bool">bool</option><option value="string">string</option><option value="number">number</option><option value="json">json</option></select></label><Field label="Default value"><TypedValue type={d.value_type ?? 'bool'} value={d.default_value} onChange={(v) => set({ default_value: v })} compact /></Field><div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div></div>) },
        { label: 'Targeting', render: (d, set) => (<><div className="label" style={{ margin: '4px 0 6px' }}>Rules <span className="hint">— evaluated top to bottom, first match wins; leave empty to serve the default to everyone</span></div><RulesEditor type={d.value_type ?? 'bool'} value={d.rules ?? []} onChange={(rules) => set({ rules })} /></>) },
      ]}
      tabs={[
        { id: 'overrides', label: 'Overrides', render: (f, reload) => <Overrides flag={f} reload={reload} /> },
        { id: 'explain', label: 'Explain', render: (f) => <Explain flag={f} /> },
        { id: 'diff', label: 'Diff & size', render: (f) => <Diff flag={f} /> },
        { id: 'dryrun', label: 'Dry run', render: (f) => <DryRun flag={f} /> },
      ]}
    />
  );
}
