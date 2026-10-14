import { useState } from 'react';
import { get, post } from '../api';
import { EntityPage, Text, Num, Tags, CONDITION_HINT } from '../crud';
import { Badge, ErrorBox, Field, JsonField, Toggle, fmtDate } from '../ui';

type Audience = { key: string; name: string; description: string; condition: unknown; percentage: number; enabled: boolean; tags: string[]; version: number; updated_at: string };

function Check({ aud }: { aud: Audience }) {
  const [ext, setExt] = useState('');
  const [props, setProps] = useState<unknown>({ default: { country: 'DE' }, custom: {} });
  const [out, setOut] = useState<string | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const byId = async () => {
    try {
      const id = await get<{ id: string }>(`/identities/by-external-id/${encodeURIComponent(ext)}`);
      const r = await get<{ member: boolean }>(`/audiences/${encodeURIComponent(aud.key)}/evaluate/${id.id}`);
      setOut(`${ext} is ${r.member ? '' : 'NOT '}a member`); setErr(null);
    } catch (e) { setErr(e as Error); }
  };
  const dry = () => post<{ member: boolean }>(`/audiences/${encodeURIComponent(aud.key)}/evaluate`, { properties: props }).then((r) => { setOut(`hypothetical identity is ${r.member ? '' : 'NOT '}a member`); setErr(null); }, setErr);
  return (
    <div className="card">
      <strong>Membership check</strong>
      <ErrorBox error={err} />
      <div className="row" style={{ marginTop: 10 }}>
        <Field label="External id"><input value={ext} onChange={(e) => setExt(e.target.value)} placeholder="player-123" /></Field>
        <div className="field fixed"><span className="label">&nbsp;</span><button className="btn ghost" disabled={!ext} onClick={byId}>Check</button></div>
      </div>
      <Field label="…or hypothetical properties"><JsonField value={props} onChange={setProps} rows={4} /></Field>
      <button className="btn ghost" onClick={dry}>Dry run</button>
      {out && <div style={{ marginTop: 10 }}><Badge tone={out.includes('NOT') ? 'bad' : 'ok'}>{out}</Badge></div>}
    </div>
  );
}

export default function Audiences() {
  return (
    <EntityPage<Audience>
      title="Audiences"
      path="/audiences"
      blurb="Reusable segments defined by rules over identity properties. Reference them from flags, experiments, live events and campaigns."
      cols={[
        { h: 'Key', c: (a) => <><strong>{a.key}</strong><div className="hint">{a.name}</div></> },
        { h: 'Sampling', c: (a) => `${a.percentage}%` },
        { h: 'Status', c: (a) => a.enabled ? <Badge tone="ok">enabled</Badge> : <Badge>disabled</Badge> },
        { h: 'Updated', c: (a) => fmtDate(a.updated_at) },
      ]}
      blank={() => ({ key: '', name: '', description: '', condition: { all: [] }, percentage: 100, enabled: true, tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} />
          </div>
          <Text label="Description" value={d.description} onChange={(description) => set({ description })} />
          <Field label="Condition" hint={CONDITION_HINT}><JsonField value={d.condition} onChange={(condition) => set({ condition })} rows={8} /></Field>
          <div className="row">
            <Num label="Sampling %" value={d.percentage} onChange={(percentage) => set({ percentage: percentage ?? 100 })} min={0} max={100} hint="deterministic per identity" />
            <div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(a) => <Check aud={a} />}
    />
  );
}
