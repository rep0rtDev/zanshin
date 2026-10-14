import { useState } from 'react';
import { api, get, post, put, useLoad } from '../api';
import { EntityPage, Text, Num, Tags } from '../crud';
import { ConditionEditor } from '../RuleBuilder';
import type { Cond } from '../RuleBuilder';
import { Badge, Confirm, ErrorBox, Field, JsonField, Table, Toggle, fmtDate, fmtNum } from '../ui';

type Audience = {
  key: string; name: string; description: string; condition: Cond; percentage: number; enabled: boolean; tags: string[]; version: number; updated_at: string;
  kind: 'dynamic' | 'managed'; recompute: 'live' | 'scheduled'; schedule_cron: string; source: string; copied_from?: string | null; member_count: number; last_computed_at: string | null;
};
type Member = { identity_id: string; external_id: string; joined_at: string; source: string };
type ListEntry = { identity_id: string; external_id: string; mode: string; until: string | null; reason: string; created_at: string };
type UsageRef = { kind: string; key: string; name: string };

function ExpressionEditor({ value, onChange }: { value: Cond; onChange: (c: Cond) => void }) {
  const [mode, setMode] = useState<'builder' | 'expr'>('builder');
  const [text, setText] = useState('');
  const [err, setErr] = useState<Error | null>(null);
  const toExpr = () => post<{ expression: string }>('/audiences/expression', { condition: value }).then((r) => { setText(r.expression); setMode('expr'); setErr(null); }, setErr);
  const apply = () => post<{ condition: Cond }>('/audiences/expression', { expression: text }).then((r) => { onChange(r.condition); setMode('builder'); setErr(null); }, setErr);
  return (
    <Field label={<>Condition <a href="#" className="hint" onClick={(e) => { e.preventDefault(); mode === 'builder' ? toExpr() : setMode('builder'); }}>{mode === 'builder' ? 'edit as expression' : 'back to builder'}</a></>} hint="who belongs to this audience">
      <ErrorBox error={err} />
      {mode === 'builder'
        ? <ConditionEditor value={value} onChange={(condition) => onChange(condition ?? { all: [] })} />
        : <>
          <textarea className="mono" rows={4} value={text} onChange={(e) => setText(e.target.value)} placeholder='custom.level >= 10 and (default.country in ["DE", "FR"] or computed.session_count > 3) and computed.last_seen_at within_last 7d' />
          <div className="actions"><button className="btn" type="button" onClick={apply}>Apply expression</button></div>
        </>}
    </Field>
  );
}

function Members({ aud }: { aud: Audience }) {
  const k = encodeURIComponent(aud.key);
  const q = useLoad(() => get<{ items: Member[] }>(`/audiences/${k}/members?limit=200`), [aud.key, aud.member_count]);
  const lists = useLoad(() => get<ListEntry[]>(`/audiences/${k}/lists`), [aud.key]);
  const usage = useLoad(() => get<UsageRef[]>(`/audiences/${k}/usage`), [aud.key]);
  const [ext, setExt] = useState('');
  const [mode, setMode] = useState<'include' | 'exclude' | 'managed'>(aud.kind === 'managed' ? 'managed' : 'include');
  const [hours, setHours] = useState('');
  const [reason, setReason] = useState('');
  const [copyKey, setCopyKey] = useState(aud.key + '_copy');
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const run = (f: () => Promise<unknown>, done?: string) => f().then(() => { setErr(null); setMsg(done ?? null); q.reload(); lists.reload(); }, setErr);
  const add = () => mode === 'managed'
    ? run(() => post(`/audiences/${k}/members`, { external_ids: ext.split(/[\s,]+/).filter(Boolean) }), 'members added')
    : run(() => put(`/audiences/${k}/lists`, { external_id: ext, mode, reason, until: hours ? new Date(Date.now() + Number(hours) * 3600e3).toISOString() : null }), `${mode}d ${ext}`);
  return (
    <>
      <div className="card">
        <div className="row" style={{ alignItems: 'center' }}>
          <strong>Members</strong> <Badge tone="info">{fmtNum(aud.member_count)}</Badge>
          <span className="hint">{aud.last_computed_at ? `computed ${fmtDate(aud.last_computed_at)}` : aud.recompute === 'live' ? 'mirrored from live evaluation' : 'never computed'}</span>
          <div className="spacer" />
          {aud.kind === 'dynamic' && <button className="btn ghost" onClick={() => run(() => post(`/audiences/${k}/recompute`), 'recomputed')}>Recompute now</button>}
        </div>
        <ErrorBox error={err} />
        {msg && <div className="hint">{msg}</div>}
        <div className="row" style={{ alignItems: 'end', marginTop: 10 }}>
          <Field label={mode === 'managed' ? 'External ids (comma / space separated)' : 'External id'}><input value={ext} onChange={(e) => setExt(e.target.value)} placeholder="player-123" /></Field>
          <Field label="Action">
            <select value={mode} onChange={(e) => setMode(e.target.value as typeof mode)}>
              {aud.kind === 'managed' && <option value="managed">add as managed members</option>}
              <option value="include">always include</option>
              <option value="exclude">exclude</option>
            </select>
          </Field>
          {mode === 'exclude' && <Field label="For hours (blank = forever)"><input value={hours} onChange={(e) => setHours(e.target.value)} style={{ width: 90 }} /></Field>}
          {mode !== 'managed' && <Field label="Reason"><input value={reason} onChange={(e) => setReason(e.target.value)} /></Field>}
          <button className="btn" disabled={!ext} onClick={add}>Apply</button>
        </div>
        <Table<Member>
          rows={q.data?.items}
          rowKey={(m) => m.identity_id}
          empty={aud.recompute === 'scheduled' ? 'No stored members — run a recompute' : 'No members recorded yet'}
          cols={[
            { h: 'Player', c: (m) => <span className="mono">{m.external_id}</span> },
            { h: 'Joined', c: (m) => fmtDate(m.joined_at) },
            { h: 'Source', w: '110px', c: (m) => <Badge tone={m.source === 'include' ? 'info' : 'muted'}>{m.source}</Badge> },
            { h: '', w: '90px', c: (m) => aud.kind === 'managed' ? <a href="#" onClick={(e) => { e.preventDefault(); run(() => fetchRemove(k, m.external_id), 'member removed'); }}>remove</a> : null },
          ]}
        />
      </div>
      <div className="card">
        <strong>Include / exclude lists</strong>
        <Table<ListEntry>
          rows={lists.data}
          rowKey={(l) => l.identity_id}
          empty="No manual entries"
          cols={[
            { h: 'Player', c: (l) => <span className="mono">{l.external_id}</span> },
            { h: 'Mode', w: '100px', c: (l) => <Badge tone={l.mode === 'include' ? 'ok' : 'bad'}>{l.mode}</Badge> },
            { h: 'Until', c: (l) => l.until ? fmtDate(l.until) : <span className="muted">permanent</span> },
            { h: 'Reason', c: (l) => <span className="hint">{l.reason}</span> },
            { h: '', w: '90px', c: (l) => <a href="#" onClick={(e) => { e.preventDefault(); run(() => fetchUnset(k, l.external_id), 'entry removed'); }}>remove</a> },
          ]}
        />
      </div>
      <div className="card">
        <div className="row" style={{ alignItems: 'end' }}>
          <div className="grow">
            <strong>Used by</strong>
            <div style={{ marginTop: 6 }}>{usage.data?.length ? usage.data.map((u) => <Badge key={u.kind + u.key}>{u.kind}: {u.key}</Badge>) : <span className="muted">Not referenced by any flag, experiment, live event or campaign</span>}</div>
          </div>
          <Field label="Copy as"><input className="mono" value={copyKey} onChange={(e) => setCopyKey(e.target.value)} /></Field>
          <Confirm onConfirm={() => run(() => post(`/audiences/${k}/copy`, { key: copyKey }), `copied to ${copyKey} (disabled)`)}>Copy audience</Confirm>
        </div>
      </div>
    </>
  );
}

const fetchRemove = (k: string, externalId: string) => api('DELETE', `/audiences/${k}/members`, { external_ids: [externalId] });
const fetchUnset = (k: string, externalId: string) => api('DELETE', `/audiences/${k}/lists`, { external_id: externalId });

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
        { h: 'Type', c: (a) => <>{a.kind === 'managed' ? <Badge tone="info">managed</Badge> : <Badge>{a.recompute === 'scheduled' ? 'scheduled' : 'live'}</Badge>} {a.copied_from && <span className="hint">copy of {a.copied_from}</span>}</> },
        { h: 'Members', c: (a) => fmtNum(a.member_count) },
        { h: 'Sampling', c: (a) => `${a.percentage}%` },
        { h: 'Status', c: (a) => a.enabled ? <Badge tone="ok">enabled</Badge> : <Badge>disabled</Badge> },
        { h: 'Updated', c: (a) => fmtDate(a.updated_at) },
      ]}
      blank={() => ({ key: '', name: '', description: '', condition: { all: [] }, percentage: 100, enabled: true, tags: [], kind: 'dynamic', recompute: 'live', schedule_cron: '', source: '', member_count: 0, last_computed_at: null })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} />
          </div>
          <Text label="Description" value={d.description} onChange={(description) => set({ description })} />
          <div className="row">
            <Field label="Kind" hint="managed: membership pushed via API / console, no rule">
              <select value={d.kind ?? 'dynamic'} onChange={(e) => set({ kind: e.target.value as Audience['kind'] })}><option value="dynamic">dynamic (rule)</option><option value="managed">managed</option></select>
            </Field>
            {d.kind !== 'managed' && (
              <Field label="Evaluation" hint="live: rule checked on every request · scheduled: stored membership refreshed by cron">
                <select value={d.recompute ?? 'live'} onChange={(e) => set({ recompute: e.target.value as Audience['recompute'] })}><option value="live">live</option><option value="scheduled">scheduled</option></select>
              </Field>
            )}
            {d.kind !== 'managed' && d.recompute === 'scheduled' && <Text label="Cron (UTC, 6 fields)" value={d.schedule_cron} onChange={(schedule_cron) => set({ schedule_cron })} mono placeholder="0 0 * * * *" />}
            {d.kind === 'managed' && <Text label="Source" value={d.source} onChange={(source) => set({ source })} placeholder="nakama:guilds" />}
          </div>
          {d.kind !== 'managed' && <ExpressionEditor value={d.condition ?? { all: [] }} onChange={(condition) => set({ condition })} />}
          <div className="row">
            <Num label="Sampling %" value={d.percentage} onChange={(percentage) => set({ percentage: percentage ?? 100 })} min={0} max={100} hint="deterministic per identity" />
            <div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(a) => <><Members aud={a} /><Check aud={a} /></>}
    />
  );
}
