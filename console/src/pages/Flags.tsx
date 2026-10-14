import { useState } from 'react';
import { get, post, put, useLoad, del } from '../api';
import { EntityPage, Text, Tags, CONDITION_HINT } from '../crud';
import { Badge, Confirm, ErrorBox, Field, JsonField, Table, Toggle, fmtDate } from '../ui';

type Flag = {
  key: string; name: string; description: string; value_type: 'bool' | 'string' | 'number' | 'json'; default_value: unknown;
  enabled: boolean; rules: unknown[]; tags: string[]; version: number; updated_at: string;
};
type Override = { identity_id: string; external_id: string; value: unknown };

const RULES_HINT = (
  <>
    Array of rules, first match wins. Each: <code>{'{"id":"r1","name":"beta","condition":{…},"audiences":["beta"],"percentage":100,"value":true}'}</code>{' '}
    or weighted <code>{'"variants":[{"key":"a","value":1,"weight":50},…]'}</code> instead of <code>value</code>. {CONDITION_HINT}
  </>
);

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
        { h: 'Rules', c: (f) => f.rules.length },
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
              <select value={d.value_type} onChange={(e) => set({ value_type: e.target.value as Flag['value_type'] })}>
                <option value="bool">bool</option><option value="string">string</option><option value="number">number</option><option value="json">json</option>
              </select>
            </label>
            <Field label="Default value (JSON)"><JsonField value={d.default_value} onChange={(v) => set({ default_value: v })} rows={1} /></Field>
            <div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <Field label="Rules" hint={RULES_HINT}><JsonField value={d.rules} onChange={(v) => set({ rules: (v as unknown[]) ?? [] })} rows={10} /></Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(f, reload) => (<><Overrides flag={f} reload={reload} /><DryRun flag={f} /></>)}
    />
  );
}
