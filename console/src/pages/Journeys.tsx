import { useState } from 'react';
import { get, post, useLoad } from '../api';
import { DateTime, EntityPage, Num, Tags, Text } from '../crud';
import { AudiencePicker, ConditionEditor } from '../RuleBuilder';
import type { Cond } from '../RuleBuilder';
import { Badge, ErrorBox, Field, JsonField, Table, Toggle, fmtDate, fmtNum } from '../ui';

type Trigger = { kind: 'manual' } | { kind: 'event'; name: string } | { kind: 'identity_created' } | { kind: 'audience_enter'; audience: string } | { kind: 'live_event_start'; live_event: string };
type Step = { id: string; kind: string } & Record<string, unknown>;
type Journey = {
  id: string; key: string; name: string; description: string; enabled: boolean; trigger: Trigger; condition: Cond | null; audiences: string[]; steps: Step[];
  cooldown_secs: number; max_entries: number | null; starts_at: string | null; ends_at: string | null; tags: string[]; version: number; updated_at: string;
};
type Run = { id: string; external_id: string; identity_id: string; step: number; status: 'active' | 'done' | 'exited' | 'failed'; wake_at: string | null; error: string; entered_at: string; updated_at: string };

const KINDS: Record<string, string> = {
  message: 'Send inbox message', track: 'Track event', flag_override: 'Set flag override', experiment_assign: 'Assign experiment variant',
  integration: 'Send via integration', wait: 'Wait (seconds)', wait_until: 'Wait until local time', branch: 'Branch on condition', goto: 'Go to step', exit: 'Exit',
};
const TONE: Record<Run['status'], 'ok' | 'bad' | 'warn' | 'info'> = { done: 'ok', failed: 'bad', exited: 'warn', active: 'info' };

function blankStep(kind: string, i: number): Step {
  const id = `${kind}_${i}`;
  switch (kind) {
    case 'message': return { id, kind, title: '', body: '', image_url: '', payload: null, template_key: null, push: false };
    case 'track': return { id, kind, name: '', value: '', metadata: {} };
    case 'flag_override': return { id, kind, flag: '', value: true };
    case 'experiment_assign': return { id, kind, experiment: '', variant: '' };
    case 'integration': return { id, kind, integration: '', title: '', body: '', payload: {} };
    case 'wait': return { id, kind, secs: 3600 };
    case 'wait_until': return { id, kind, hour: 9, minute: 0, timezone_prop: 'default.timezone' };
    case 'branch': return { id, kind, condition: { all: [] }, then: '', else: null };
    case 'goto': return { id, kind, step: '' };
    default: return { id, kind: 'exit' };
  }
}

function TriggerEditor({ value, onChange }: { value: Trigger; onChange: (t: Trigger) => void }) {
  return (
    <div className="row" style={{ alignItems: 'end' }}>
      <Field label="Trigger">
        <select value={value.kind} onChange={(e) => { const k = e.target.value as Trigger['kind']; onChange(k === 'event' ? { kind: 'event', name: '' } : k === 'audience_enter' ? { kind: 'audience_enter', audience: '' } : k === 'live_event_start' ? { kind: 'live_event_start', live_event: '' } : { kind: k } as Trigger); }}>
          <option value="manual">manual (API / server only)</option><option value="event">player tracks an event</option><option value="identity_created">new player created</option>
          <option value="audience_enter">player enters an audience</option><option value="live_event_start">live event starts</option>
        </select>
      </Field>
      {value.kind === 'event' && <Text label="Event name" value={value.name} onChange={(name) => onChange({ kind: 'event', name })} mono placeholder="purchase" />}
      {value.kind === 'audience_enter' && <Text label="Audience key" value={value.audience} onChange={(audience) => onChange({ kind: 'audience_enter', audience })} mono />}
      {value.kind === 'live_event_start' && <Text label="Live event key" value={value.live_event} onChange={(live_event) => onChange({ kind: 'live_event_start', live_event })} mono />}
    </div>
  );
}

function StepEditor({ step, ids, onChange }: { step: Step; ids: string[]; onChange: (s: Step) => void }) {
  const s = step as Record<string, unknown>;
  const str = (k: string) => (s[k] as string) ?? '';
  const set = (patch: Record<string, unknown>) => onChange({ ...step, ...patch });
  const target = (label: string, k: string, optional?: boolean) => (
    <Field label={label}><select value={str(k)} onChange={(e) => set({ [k]: e.target.value || (optional ? null : '') })}>{optional && <option value="">— next step —</option>}{!optional && <option value="">—</option>}{ids.filter((i) => i !== step.id).map((i) => <option key={i} value={i}>{i}</option>)}</select></Field>
  );
  switch (step.kind) {
    case 'message': return <>
      <Text label="Title" value={str('title')} onChange={(title) => set({ title })} placeholder="Thanks, {{custom.name}}!" />
      <Field label="Body"><textarea rows={2} value={str('body')} onChange={(e) => set({ body: e.target.value })} style={{ fontFamily: 'inherit' }} /></Field>
      <div className="row"><Text label="Template key (optional)" value={str('template_key')} onChange={(v) => set({ template_key: v || null })} mono /><Text label="Image URL" value={str('image_url')} onChange={(image_url) => set({ image_url })} /><div className="field fixed"><span className="label">Push</span><Toggle value={!!s.push} onChange={(push) => set({ push })} label={s.push ? 'yes' : 'no'} /></div></div>
      <Field label="Payload (JSON)"><JsonField value={s.payload} onChange={(payload) => set({ payload: payload ?? null })} rows={1} /></Field>
    </>;
    case 'track': return <div className="row"><Text label="Event name" value={str('name')} onChange={(name) => set({ name })} mono /><Text label="Value" value={str('value')} onChange={(value) => set({ value })} /><Field label="Metadata (JSON)"><JsonField value={s.metadata} onChange={(metadata) => set({ metadata: metadata ?? {} })} rows={1} /></Field></div>;
    case 'flag_override': return <div className="row"><Text label="Flag key" value={str('flag')} onChange={(flag) => set({ flag })} mono /><Field label="Value (JSON)"><JsonField value={s.value} onChange={(value) => set({ value })} rows={1} /></Field></div>;
    case 'experiment_assign': return <div className="row"><Text label="Experiment key" value={str('experiment')} onChange={(experiment) => set({ experiment })} mono /><Text label="Variant" value={str('variant')} onChange={(variant) => set({ variant })} mono /></div>;
    case 'integration': return <>
      <div className="row"><Text label="Integration key" value={str('integration')} onChange={(integration) => set({ integration })} mono /><Text label="Title" value={str('title')} onChange={(title) => set({ title })} /></div>
      <Field label="Body"><textarea rows={2} value={str('body')} onChange={(e) => set({ body: e.target.value })} style={{ fontFamily: 'inherit' }} /></Field>
      <Field label="Payload (JSON)"><JsonField value={s.payload} onChange={(payload) => set({ payload: payload ?? {} })} rows={1} /></Field>
    </>;
    case 'wait': return <Num label="Seconds" value={(s.secs as number) ?? 0} onChange={(secs) => set({ secs: secs ?? 1 })} min={1} hint="e.g. 86400 = one day" />;
    case 'wait_until': return <div className="row"><Num label="Hour" value={(s.hour as number) ?? 9} onChange={(hour) => set({ hour: hour ?? 9 })} min={0} max={23} /><Num label="Minute" value={(s.minute as number) ?? 0} onChange={(minute) => set({ minute: minute ?? 0 })} min={0} max={59} /><Text label="Timezone property" value={str('timezone_prop')} onChange={(timezone_prop) => set({ timezone_prop })} mono hint="IANA zone read from the player, UTC fallback" /></div>;
    case 'branch': return <>
      <Field label="Condition"><ConditionEditor value={(s.condition as Cond) ?? { all: [] }} onChange={(condition) => set({ condition })} /></Field>
      <div className="row">{target('Then go to', 'then')}{target('Else go to', 'else', true)}</div>
    </>;
    case 'goto': return target('Go to step', 'step');
    default: return <div className="hint">The player leaves the journey here.</div>;
  }
}

function StepsEditor({ value, onChange }: { value: Step[]; onChange: (v: Step[]) => void }) {
  const [adding, setAdding] = useState('message');
  const ids = value.map((s) => s.id);
  const move = (i: number, d: number) => { const v = [...value]; const [s] = v.splice(i, 1); v.splice(i + d, 0, s); onChange(v); };
  return (
    <div className="stack">
      {value.map((s, i) => (
        <div key={i} className="card" style={{ padding: 10 }}>
          <div className="row" style={{ alignItems: 'end' }}>
            <span className="hint" style={{ width: 24 }}>{i + 1}.</span>
            <Text label="Step id" value={s.id} onChange={(id) => onChange(value.map((x, j) => (j === i ? { ...x, id } : x)))} mono />
            <Field label="Action"><select value={s.kind} onChange={(e) => onChange(value.map((x, j) => (j === i ? { ...blankStep(e.target.value, i), id: x.id } : x)))}>{Object.keys(KINDS).map((k) => <option key={k} value={k}>{KINDS[k]}</option>)}</select></Field>
            <button className="btn ghost sm" disabled={i === 0} onClick={() => move(i, -1)}>↑</button>
            <button className="btn ghost sm" disabled={i === value.length - 1} onClick={() => move(i, 1)}>↓</button>
            <button className="btn ghost sm" onClick={() => onChange(value.filter((_, j) => j !== i))}>Remove</button>
          </div>
          <StepEditor step={s} ids={ids} onChange={(ns) => onChange(value.map((x, j) => (j === i ? ns : x)))} />
        </div>
      ))}
      <div className="row" style={{ alignItems: 'end' }}>
        <Field label="Add step"><select value={adding} onChange={(e) => setAdding(e.target.value)}>{Object.keys(KINDS).map((k) => <option key={k} value={k}>{KINDS[k]}</option>)}</select></Field>
        <button className="btn ghost" onClick={() => onChange([...value, blankStep(adding, value.length + 1)])}>Add</button>
      </div>
    </div>
  );
}

function Runs({ j }: { j: Journey }) {
  const [status, setStatus] = useState('');
  const [ext, setExt] = useState('');
  const [err, setErr] = useState<Error | null>(null);
  const k = encodeURIComponent(j.key);
  const q = useLoad(() => get<{ items: Run[]; stats: { status: string; count: number }[]; steps: { index: number; id: string; active: number }[] }>(`/journeys/${k}/runs?limit=100${status ? '&status=' + status : ''}`), [j.key, j.version, status]);
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}>
        <strong>Runs</strong>
        <select value={status} onChange={(e) => setStatus(e.target.value)}><option value="">all</option><option value="active">active</option><option value="done">done</option><option value="exited">exited</option><option value="failed">failed</option></select>
        <button className="btn ghost sm" onClick={q.reload}>Refresh</button>
        <div className="row" style={{ marginLeft: 'auto', alignItems: 'end' }}>
          <Text label="Enter a player (external id)" value={ext} onChange={setExt} mono />
          <button className="btn sm" disabled={!ext} onClick={() => post(`/journeys/${k}/enter`, { external_id: ext }).then(() => { setErr(null); q.reload(); }, setErr)}>Enter</button>
        </div>
      </div>
      <ErrorBox error={err ?? q.error} />
      <div className="chips" style={{ marginTop: 8 }}>{(q.data?.stats ?? []).map((s) => <span key={s.status} className="chip">{s.status}: {fmtNum(s.count)}</span>)}</div>
      {q.data && q.data.steps.some((s) => s.active > 0) && (
        <div className="chips" style={{ marginTop: 6 }}>{q.data.steps.map((s) => <span key={s.id} className={'chip' + (s.active ? ' on' : '')}>{s.index + 1}. {s.id}{s.active ? ` · ${fmtNum(s.active)} waiting` : ''}</span>)}</div>
      )}
      <Table<Run>
        rows={q.data?.items}
        rowKey={(r) => r.id}
        empty="Nobody has entered yet"
        cols={[
          { h: 'Player', c: (r) => <span className="mono">{r.external_id}</span> },
          { h: 'Status', c: (r) => <><Badge tone={TONE[r.status]}>{r.status}</Badge> <span className="hint">step {r.step + 1}</span></> },
          { h: 'Entered', c: (r) => fmtDate(r.entered_at) },
          { h: 'Detail', c: (r) => <span className="hint">{r.error || (r.status === 'active' && r.wake_at ? `wakes ${fmtDate(r.wake_at)}` : '')}</span> },
          { h: '', c: (r) => r.status === 'active' ? <button className="btn ghost sm" onClick={() => post(`/journeys/${k}/runs/${r.id}/exit`).then(q.reload, setErr)}>Exit</button> : null },
        ]}
      />
    </div>
  );
}

export default function Journeys() {
  return (
    <EntityPage<Journey>
      title="Journeys"
      path="/journeys"
      blurb="Multi-step automations per player: a trigger enters them, then messages, events, flag overrides, experiment assignments, waits (fixed or until a local time) and branches run in order. Waits are persisted, so they survive restarts."
      cols={[
        { h: 'Key', c: (j) => <><strong>{j.key}</strong><div className="hint">{j.name}</div></> },
        { h: 'Trigger', c: (j) => <code>{j.trigger.kind}{'name' in j.trigger ? ` ${j.trigger.name}` : ''}{'audience' in j.trigger ? ` ${j.trigger.audience}` : ''}{'live_event' in j.trigger ? ` ${j.trigger.live_event}` : ''}</code> },
        { h: 'Steps', c: (j) => j.steps.length },
        { h: 'Enabled', c: (j) => j.enabled ? <Badge tone="ok">on</Badge> : <Badge>off</Badge> },
        { h: 'Updated', c: (j) => fmtDate(j.updated_at) },
      ]}
      blank={() => ({ key: '', name: '', description: '', enabled: true, trigger: { kind: 'event', name: '' }, condition: null, audiences: [], steps: [blankStep('message', 1)], cooldown_secs: 0, max_entries: null, starts_at: null, ends_at: null, tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} />
            <div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <Text label="Description" value={d.description} onChange={(description) => set({ description })} />
          <TriggerEditor value={d.trigger ?? { kind: 'manual' }} onChange={(trigger) => set({ trigger })} />
          <Field label="Steps"><StepsEditor value={d.steps ?? []} onChange={(steps) => set({ steps })} /></Field>
          <div className="row">
            <Num label="Cooldown (s)" value={d.cooldown_secs} onChange={(cooldown_secs) => set({ cooldown_secs: cooldown_secs ?? 0 })} min={0} hint="min. time between two entries of the same player" />
            <Num label="Max entries per player" value={d.max_entries ?? null} onChange={(max_entries) => set({ max_entries: max_entries ?? null })} min={1} hint="blank = unlimited" />
          </div>
          <Field label="Entry condition"><ConditionEditor optional value={d.condition} onChange={(condition) => set({ condition })} /></Field>
          <Field label="Audiences"><AudiencePicker value={d.audiences ?? []} onChange={(audiences) => set({ audiences })} /></Field>
          <div className="row">
            <DateTime label="Starts" value={d.starts_at} onChange={(starts_at) => set({ starts_at })} />
            <DateTime label="Ends" value={d.ends_at} onChange={(ends_at) => set({ ends_at })} />
          </div>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(j) => <Runs j={j} />}
    />
  );
}
