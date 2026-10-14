import { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { get, post, useLoad } from '../api';
import { DateTime, EntityPage, Num, Tags, Text } from '../crud';
import { AudiencePicker, ConditionEditor } from '../RuleBuilder';
import type { Cond } from '../RuleBuilder';
import { Badge, ErrorBox, Field, JsonField, Table, Toggle, fmtDate, fmtNum } from '../ui';

type Trigger = { kind: 'manual' } | { kind: 'event'; name: string } | { kind: 'identity_created' } | { kind: 'audience_enter'; audience: string } | { kind: 'live_event_start'; live_event: string };
type Step = { id: string; kind: string } & Record<string, unknown>;
type Status = 'disabled' | 'scheduled' | 'running' | 'terminated';
type Journey = {
  id: string; key: string; name: string; description: string; enabled: boolean; trigger: Trigger; condition: Cond | null; audiences: string[]; steps: Step[];
  cooldown_secs: number; max_entries: number | null; starts_at: string | null; ends_at: string | null; tags: string[]; version: number; updated_at: string; status?: Status;
};
type Run = { id: string; external_id: string; identity_id: string; step: number; status: 'active' | 'done' | 'exited' | 'failed'; wake_at: string | null; error: string; entered_at: string; updated_at: string };
type StepStat = { index: number; id: string; active: number; passed: number };
type Timing = { completed: number; min_secs: number | null; avg_secs: number | null; max_secs: number | null };
type RunsResp = { items: Run[]; stats: { status: string; count: number }[]; steps: StepStat[]; timing: Timing; status: Status };

const KINDS: Record<string, string> = {
  message: 'Send message', track: 'Track event', flag_override: 'Set flag override', experiment_assign: 'Assign experiment variant', live_event: 'Live event (player timeline)',
  integration: 'Send via integration', wait: 'Wait (duration)', wait_until: 'Wait until local time', time_window: 'Time window (date / schedule)', branch: 'Branch on condition',
  goto: 'Go to step', revert: 'Revert earlier step', enter_journey: 'Enter another journey', exit: 'Exit',
};
const GROUP: Record<string, 'action' | 'delay' | 'flow'> = {
  message: 'action', track: 'action', flag_override: 'action', experiment_assign: 'action', live_event: 'action', integration: 'action', revert: 'action', enter_journey: 'action',
  wait: 'delay', wait_until: 'delay', time_window: 'delay', branch: 'flow', goto: 'flow', exit: 'flow',
};
const TONE: Record<Run['status'], 'ok' | 'bad' | 'warn' | 'info'> = { done: 'ok', failed: 'bad', exited: 'warn', active: 'info' };
const STATUS_TONE: Record<Status, 'ok' | 'bad' | 'warn' | 'info' | undefined> = { running: 'ok', scheduled: 'info', terminated: 'warn', disabled: undefined };

export const fmtDur = (s: number | null | undefined) => {
  if (s === null || s === undefined) return '—';
  if (s < 60) return `${Math.round(s)}s`;
  if (s < 3600) return `${(s / 60).toFixed(1)}m`;
  if (s < 86400) return `${(s / 3600).toFixed(1)}h`;
  return `${(s / 86400).toFixed(1)}d`;
};

function blankStep(kind: string, i: number): Step {
  const id = `${kind}_${i}`;
  switch (kind) {
    case 'message': return { id, kind, title: '', body: '', image_url: '', payload: null, template_key: null, push: false };
    case 'track': return { id, kind, name: '', value: '', metadata: {} };
    case 'flag_override': return { id, kind, flag: '', value: true };
    case 'experiment_assign': return { id, kind, experiment: '', variant: '' };
    case 'live_event': return { id, kind, live_event: '', duration_secs: 172800 };
    case 'integration': return { id, kind, integration: '', title: '', body: '', payload: {} };
    case 'wait': return { id, kind, secs: 3600 };
    case 'wait_until': return { id, kind, hour: 9, minute: 0, timezone_prop: 'default.timezone' };
    case 'time_window': return { id, kind, at: null, cron: '0 0 9 * * Sat', timezone: 'player', timezone_prop: 'default.timezone' };
    case 'branch': return { id, kind, condition: { all: [] }, then: '', else: null };
    case 'goto': return { id, kind, step: '' };
    case 'revert': return { id, kind, step: '' };
    case 'enter_journey': return { id, kind, journey: '' };
    default: return { id, kind: 'exit' };
  }
}

function summary(s: Step): string {
  const v = (k: string) => String(s[k] ?? '');
  switch (s.kind) {
    case 'message': return v('template_key') ? `template ${v('template_key')}` : v('title') || 'inbox message';
    case 'track': return v('name') || 'event';
    case 'flag_override': return `${v('flag')} = ${JSON.stringify(s.value)}`;
    case 'experiment_assign': return `${v('experiment')} → ${v('variant')}`;
    case 'live_event': return `${v('live_event')} for ${fmtDur(s.duration_secs as number)}`;
    case 'integration': return v('integration');
    case 'wait': return fmtDur(s.secs as number);
    case 'wait_until': return `${String(s.hour).padStart(2, '0')}:${String(s.minute ?? 0).padStart(2, '0')} local`;
    case 'time_window': return s.at ? fmtDate(s.at as string) : `${v('cron')} (${v('timezone') || 'UTC'})`;
    case 'branch': return `then → ${v('then') || '?'}, else → ${v('else') || 'next'}`;
    case 'goto': return `→ ${v('step')}`;
    case 'revert': return `undo ${v('step')}`;
    case 'enter_journey': return v('journey');
    default: return 'player leaves';
  }
}

function TriggerEditor({ value, onChange }: { value: Trigger; onChange: (t: Trigger) => void }) {
  return (
    <div className="row" style={{ alignItems: 'end' }}>
      <Field label="Entry trigger">
        <select value={value.kind} onChange={(e) => { const k = e.target.value as Trigger['kind']; onChange(k === 'event' ? { kind: 'event', name: '' } : k === 'audience_enter' ? { kind: 'audience_enter', audience: '' } : k === 'live_event_start' ? { kind: 'live_event_start', live_event: '' } : { kind: k } as Trigger); }}>
          <option value="manual">manual (API / server / another journey)</option><option value="event">player tracks an event</option><option value="identity_created">new player created</option>
          <option value="audience_enter">player enters an audience</option><option value="live_event_start">live event starts</option>
        </select>
      </Field>
      {value.kind === 'event' && <Text label="Event name" value={value.name} onChange={(name) => onChange({ kind: 'event', name })} mono placeholder="purchase" />}
      {value.kind === 'audience_enter' && <Text label="Audience key" value={value.audience} onChange={(audience) => onChange({ kind: 'audience_enter', audience })} mono />}
      {value.kind === 'live_event_start' && <Text label="Live event key" value={value.live_event} onChange={(live_event) => onChange({ kind: 'live_event_start', live_event })} mono />}
    </div>
  );
}

function StepEditor({ step, steps, onChange }: { step: Step; steps: Step[]; onChange: (s: Step) => void }) {
  const s = step as Record<string, unknown>;
  const str = (k: string) => (s[k] as string) ?? '';
  const set = (patch: Record<string, unknown>) => onChange({ ...step, ...patch });
  const target = (label: string, k: string, optional?: boolean, filter?: (x: Step) => boolean) => (
    <Field label={label}><select value={str(k)} onChange={(e) => set({ [k]: e.target.value || (optional ? null : '') })}>{optional && <option value="">— next step —</option>}{!optional && <option value="">—</option>}{steps.filter((x) => x.id !== step.id && (!filter || filter(x))).map((x) => <option key={x.id} value={x.id}>{x.id}</option>)}</select></Field>
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
    case 'live_event': return <div className="row"><Text label="Live event key" value={str('live_event')} onChange={(live_event) => set({ live_event })} mono hint="starts for this player the moment they reach the step, whatever the event schedule" /><Num label="Duration (s)" value={(s.duration_secs as number) ?? null} onChange={(duration_secs) => set({ duration_secs: duration_secs ?? null })} min={1} hint="blank = the event's own duration" /></div>;
    case 'integration': return <>
      <div className="row"><Text label="Integration key" value={str('integration')} onChange={(integration) => set({ integration })} mono /><Text label="Title" value={str('title')} onChange={(title) => set({ title })} /></div>
      <Field label="Body"><textarea rows={2} value={str('body')} onChange={(e) => set({ body: e.target.value })} style={{ fontFamily: 'inherit' }} /></Field>
      <Field label="Payload (JSON)"><JsonField value={s.payload} onChange={(payload) => set({ payload: payload ?? {} })} rows={1} /></Field>
    </>;
    case 'wait': return <Num label="Seconds" value={(s.secs as number) ?? 0} onChange={(secs) => set({ secs: secs ?? 1 })} min={1} hint="counted from the moment this player arrives — e.g. 86400 = one day" />;
    case 'wait_until': return <div className="row"><Num label="Hour" value={(s.hour as number) ?? 9} onChange={(hour) => set({ hour: hour ?? 9 })} min={0} max={23} /><Num label="Minute" value={(s.minute as number) ?? 0} onChange={(minute) => set({ minute: minute ?? 0 })} min={0} max={59} /><Text label="Timezone property" value={str('timezone_prop')} onChange={(timezone_prop) => set({ timezone_prop })} mono hint="IANA zone read from the player, UTC fallback" /></div>;
    case 'time_window': {
      const mode = s.at ? 'at' : 'cron';
      return <>
        <div className="row" style={{ alignItems: 'end' }}>
          <Field label="Wait for"><select value={mode} onChange={(e) => (e.target.value === 'at' ? set({ at: new Date(Date.now() + 86400e3).toISOString(), cron: null }) : set({ at: null, cron: '0 0 9 * * Sat' }))}><option value="cron">next slot of a schedule</option><option value="at">a specific date</option></select></Field>
          {mode === 'at' && <DateTime label="Date" value={(s.at as string) ?? null} onChange={(at) => set({ at })} />}
          {mode === 'cron' && <Text label="Cron (sec min hour dom month dow)" value={str('cron')} onChange={(cron) => set({ cron })} mono placeholder="0 0 9 * * Sat" />}
        </div>
        {mode === 'cron' && <div className="row"><Field label="Resolve in"><select value={str('timezone') || 'UTC'} onChange={(e) => set({ timezone: e.target.value === 'UTC' ? null : e.target.value })}><option value="UTC">UTC</option><option value="player">the player's own timezone</option><option value="Europe/London">Europe/London</option><option value="Europe/Berlin">Europe/Berlin</option><option value="America/New_York">America/New_York</option><option value="America/Los_Angeles">America/Los_Angeles</option><option value="Asia/Tokyo">Asia/Tokyo</option><option value="Asia/Shanghai">Asia/Shanghai</option></select></Field>{str('timezone') === 'player' && <Text label="Timezone property" value={str('timezone_prop')} onChange={(timezone_prop) => set({ timezone_prop })} mono />}</div>}
      </>;
    }
    case 'branch': return <>
      <Field label="Condition"><ConditionEditor value={(s.condition as Cond) ?? { all: [] }} onChange={(condition) => set({ condition })} /></Field>
      <div className="row">{target('Then go to', 'then')}{target('Else go to', 'else', true)}</div>
    </>;
    case 'goto': return target('Go to step', 'step');
    case 'revert': return target('Undo step', 'step', false, (x) => ['flag_override', 'experiment_assign', 'live_event'].includes(x.kind));
    case 'enter_journey': return <Text label="Journey key" value={str('journey')} onChange={(journey) => set({ journey })} mono hint="the player is enrolled there as well (targeting and cooldown of that journey apply); journeys cannot form a cycle" />;
    default: return <div className="hint">The player leaves the journey here.</div>;
  }
}

/** Auto-laid-out flow graph of the steps with per-step player counts. */
function Canvas({ steps, stats, selected, onSelect }: { steps: Step[]; stats?: StepStat[]; selected: number; onSelect: (i: number) => void }) {
  const W = 340, H = 58, GAP = 34, X = 40, PAD = 16;
  const y = (i: number) => PAD + i * (H + GAP);
  const idx = (id: unknown) => steps.findIndex((s) => s.id === id);
  const edges = useMemo(() => {
    const out: { from: number; to: number; label?: string }[] = [];
    steps.forEach((s, i) => {
      const next = i + 1 < steps.length ? i + 1 : -1;
      if (s.kind === 'exit') return;
      if (s.kind === 'goto') { const t = idx(s.step); if (t >= 0) out.push({ from: i, to: t }); return; }
      if (s.kind === 'branch') {
        const t = idx(s.then); if (t >= 0) out.push({ from: i, to: t, label: 'yes' });
        const e = s.else ? idx(s.else) : next; if (e >= 0) out.push({ from: i, to: e, label: 'no' });
        return;
      }
      if (next >= 0) out.push({ from: i, to: next });
    });
    return out;
  }, [steps]);
  const height = PAD * 2 + steps.length * (H + GAP) - GAP + 20;
  const stat = (i: number) => stats?.find((s) => s.index === i);
  return (
    <svg className="canvas" width="100%" viewBox={`0 0 ${W + X * 2 + 160} ${height}`} style={{ maxWidth: W + X * 2 + 160 }}>
      <defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="var(--muted)" /></marker></defs>
      {edges.map((e, k) => {
        const x = X + W / 2;
        if (e.to === e.from + 1 && !e.label) return <line key={k} x1={x} y1={y(e.from) + H} x2={x} y2={y(e.to) - 2} stroke="var(--muted)" strokeWidth={1.4} markerEnd="url(#arrow)" />;
        const back = e.to <= e.from;
        const side = back ? X - 18 : X + W + 18;
        const y1 = y(e.from) + H / 2, y2 = y(e.to) + (back ? H : 0) + (back ? 2 : -2);
        const cx = back ? side - 40 : side + 40;
        const d = `M ${back ? X : X + W} ${y1} C ${cx} ${y1}, ${cx} ${y2}, ${X + W / 2 + (back ? -W / 4 : W / 4)} ${y2}`;
        return <g key={k}><path d={d} fill="none" stroke="var(--muted)" strokeWidth={1.4} strokeDasharray={e.label ? undefined : '4 3'} markerEnd="url(#arrow)" />{e.label && <text x={cx} y={(y1 + y2) / 2} fontSize={11} fill="var(--muted)" textAnchor="middle">{e.label}</text>}</g>;
      })}
      {steps.map((s, i) => {
        const st = stat(i); const g = GROUP[s.kind] ?? 'action';
        return (
          <g key={i} className={'node' + (i === selected ? ' sel' : '')} transform={`translate(${X}, ${y(i)})`} onClick={() => onSelect(i)} style={{ cursor: 'pointer' }}>
            <rect width={W} height={H} rx={8} fill="var(--card)" stroke={i === selected ? 'var(--fg)' : 'var(--line)'} strokeWidth={i === selected ? 1.8 : 1} />
            <rect width={4} height={H} rx={2} fill={g === 'delay' ? 'var(--warn)' : g === 'flow' ? 'var(--info)' : 'var(--ok)'} />
            <text x={14} y={22} fontSize={12.5} fontWeight={600} fill="var(--fg)">{i + 1}. {(KINDS[s.kind] ?? s.kind).replace(/ \(.*\)$/, '')}</text>
            <text x={14} y={41} fontSize={11.5} fill="var(--muted)">{summary(s).length > 52 ? summary(s).slice(0, 51) + '…' : summary(s)}</text>
            <text x={W - 10} y={22} fontSize={11} fill="var(--muted)" textAnchor="end" fontFamily="ui-monospace, monospace">{s.id.length > 18 ? s.id.slice(0, 17) + '…' : s.id}</text>
            {st && (st.passed > 0 || st.active > 0) && <text x={W - 10} y={41} fontSize={11} fill="var(--fg)" textAnchor="end">{fmtNum(st.passed)} passed{st.active ? ` · ${fmtNum(st.active)} waiting` : ''}</text>}
          </g>
        );
      })}
    </svg>
  );
}

function StepsEditor({ value, onChange, stats }: { value: Step[]; onChange: (v: Step[]) => void; stats?: StepStat[] }) {
  const [sel, setSel] = useState(0);
  const [adding, setAdding] = useState('message');
  useEffect(() => { if (sel >= value.length) setSel(Math.max(0, value.length - 1)); }, [value.length, sel]);
  const move = (i: number, d: number) => { const v = [...value]; const [s] = v.splice(i, 1); v.splice(i + d, 0, s); onChange(v); setSel(i + d); };
  const insert = (after: number) => { const v = [...value]; v.splice(after + 1, 0, blankStep(adding, value.length + 1)); onChange(v); setSel(after + 1); };
  const step = value[sel];
  return (
    <div className="grid" style={{ gridTemplateColumns: 'minmax(360px, 520px) 1fr', alignItems: 'start' }}>
      <div>
        <Canvas steps={value} stats={stats} selected={sel} onSelect={setSel} />
        <div className="row" style={{ alignItems: 'end', marginTop: 6 }}>
          <Field label="Add step"><select value={adding} onChange={(e) => setAdding(e.target.value)}>
            <optgroup label="Actions">{Object.keys(KINDS).filter((k) => GROUP[k] === 'action').map((k) => <option key={k} value={k}>{KINDS[k]}</option>)}</optgroup>
            <optgroup label="Delays">{Object.keys(KINDS).filter((k) => GROUP[k] === 'delay').map((k) => <option key={k} value={k}>{KINDS[k]}</option>)}</optgroup>
            <optgroup label="Flow">{Object.keys(KINDS).filter((k) => GROUP[k] === 'flow').map((k) => <option key={k} value={k}>{KINDS[k]}</option>)}</optgroup>
          </select></Field>
          <button className="btn ghost" onClick={() => insert(value.length ? sel : -1)}>Insert after selected</button>
          <button className="btn ghost" onClick={() => insert(value.length - 1)}>Append</button>
        </div>
      </div>
      {step ? (
        <div className="card" style={{ padding: 12, position: 'sticky', top: 12 }}>
          <div className="row" style={{ alignItems: 'end' }}>
            <span className="hint" style={{ width: 24 }}>{sel + 1}.</span>
            <Text label="Step id" value={step.id} onChange={(id) => onChange(value.map((x, j) => (j === sel ? { ...x, id } : x)))} mono />
            <Field label="Kind"><select value={step.kind} onChange={(e) => onChange(value.map((x, j) => (j === sel ? { ...blankStep(e.target.value, sel), id: x.id } : x)))}>{Object.keys(KINDS).map((k) => <option key={k} value={k}>{KINDS[k]}</option>)}</select></Field>
            <button className="btn ghost sm" disabled={sel === 0} onClick={() => move(sel, -1)}>↑</button>
            <button className="btn ghost sm" disabled={sel === value.length - 1} onClick={() => move(sel, 1)}>↓</button>
            <button className="btn ghost sm" onClick={() => onChange(value.filter((_, j) => j !== sel))}>Remove</button>
          </div>
          <StepEditor step={step} steps={value} onChange={(ns) => onChange(value.map((x, j) => (j === sel ? ns : x)))} />
        </div>
      ) : <div className="empty">No steps yet — add one on the left.</div>}
    </div>
  );
}

function Runs({ j }: { j: Journey }) {
  const [status, setStatus] = useState('');
  const [ext, setExt] = useState('');
  const [err, setErr] = useState<Error | null>(null);
  const k = encodeURIComponent(j.key);
  const q = useLoad(() => get<RunsResp>(`/journeys/${k}/runs?limit=100${status ? '&status=' + status : ''}`), [j.key, j.version, status]);
  const t = q.data?.timing;
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}>
        <strong>Players</strong>
        <select value={status} onChange={(e) => setStatus(e.target.value)}><option value="">all</option><option value="active">active</option><option value="done">done</option><option value="exited">exited</option><option value="failed">failed</option></select>
        <button className="btn ghost sm" onClick={q.reload}>Refresh</button>
        <div className="row" style={{ marginLeft: 'auto', alignItems: 'end' }}>
          <Text label="Enter a player (external id)" value={ext} onChange={setExt} mono />
          <button className="btn sm" disabled={!ext} onClick={() => post(`/journeys/${k}/enter`, { external_id: ext }).then(() => { setErr(null); q.reload(); }, setErr)}>Enter</button>
        </div>
      </div>
      <ErrorBox error={err ?? q.error} />
      <div className="chips" style={{ marginTop: 8 }}>
        {(q.data?.stats ?? []).map((s) => <span key={s.status} className="chip">{s.status}: {fmtNum(s.count)}</span>)}
        {t && t.completed > 0 && <span className="chip">time to finish · fastest {fmtDur(t.min_secs)} · average {fmtDur(t.avg_secs)} · slowest {fmtDur(t.max_secs)}</span>}
      </div>
      <div style={{ marginTop: 10 }}><Canvas steps={j.steps} stats={q.data?.steps} selected={-1} onSelect={() => undefined} /></div>
      <Table<Run>
        rows={q.data?.items}
        rowKey={(r) => r.id}
        empty="Nobody has entered yet"
        cols={[
          { h: 'Player', c: (r) => <Link to={`/identities/${r.identity_id}`} className="mono">{r.external_id}</Link> },
          { h: 'Status', c: (r) => <><Badge tone={TONE[r.status]}>{r.status}</Badge> <span className="hint">step {r.step + 1}</span></> },
          { h: 'Entered', c: (r) => fmtDate(r.entered_at) },
          { h: 'Detail', c: (r) => <span className="hint">{r.error || (r.status === 'active' && r.wake_at ? `wakes ${fmtDate(r.wake_at)}` : r.status === 'done' ? `finished in ${fmtDur((new Date(r.updated_at).getTime() - new Date(r.entered_at).getTime()) / 1000)}` : '')}</span> },
          { h: '', c: (r) => r.status === 'active' ? <button className="btn ghost sm" onClick={() => post(`/journeys/${k}/runs/${r.id}/exit`).then(q.reload, setErr)}>Exit</button> : null },
        ]}
      />
    </div>
  );
}

export function StatusBadge({ s }: { s?: Status }) {
  return s ? <Badge tone={STATUS_TONE[s]}>{s}</Badge> : null;
}

export default function Journeys() {
  return (
    <EntityPage<Journey>
      title="Journeys"
      path="/journeys"
      blurb="Per-player flows: an entry trigger admits the player, then actions (messages, events, flag overrides, experiment variants, live events on the player's own timeline), delays (fixed, local time, date or recurring schedule) and branches run on that player's own clock. Waits are persisted and survive restarts."
      cols={[
        { h: 'Key', c: (j) => <><strong>{j.key}</strong><div className="hint">{j.name}</div></> },
        { h: 'Trigger', c: (j) => <code>{j.trigger.kind}{'name' in j.trigger ? ` ${j.trigger.name}` : ''}{'audience' in j.trigger ? ` ${j.trigger.audience}` : ''}{'live_event' in j.trigger ? ` ${j.trigger.live_event}` : ''}</code> },
        { h: 'Steps', c: (j) => j.steps.length },
        { h: 'Status', c: (j) => <StatusBadge s={j.status} /> },
        { h: 'Updated', c: (j) => fmtDate(j.updated_at) },
      ]}
      blank={() => ({ key: '', name: '', description: '', enabled: true, trigger: { kind: 'event', name: '' }, condition: null, audiences: [], steps: [blankStep('message', 1)], cooldown_secs: 0, max_entries: null, starts_at: null, ends_at: null, tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} />
            <div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
            {!isNew && <div className="field fixed"><span className="label">Status</span><StatusBadge s={d.status} /></div>}
          </div>
          <Text label="Description" value={d.description} onChange={(description) => set({ description })} />
          <TriggerEditor value={d.trigger ?? { kind: 'manual' }} onChange={(trigger) => set({ trigger })} />
          <Field label="Flow"><StepsEditor value={d.steps ?? []} onChange={(steps) => set({ steps })} /></Field>
          <div className="row">
            <Num label="Rejoin cooldown (s)" value={d.cooldown_secs} onChange={(cooldown_secs) => set({ cooldown_secs: cooldown_secs ?? 0 })} min={0} hint="0 = a player goes through once; otherwise they become eligible again this long after finishing" />
            <Num label="Max entries per player" value={d.max_entries ?? null} onChange={(max_entries) => set({ max_entries: max_entries ?? null })} min={1} hint="blank = unlimited" />
          </div>
          <Field label="Entry condition"><ConditionEditor optional value={d.condition} onChange={(condition) => set({ condition })} /></Field>
          <Field label="Audiences"><AudiencePicker value={d.audiences ?? []} onChange={(audiences) => set({ audiences })} /></Field>
          <div className="row">
            <DateTime label="Admits players from" value={d.starts_at} onChange={(starts_at) => set({ starts_at })} />
            <DateTime label="Until" value={d.ends_at} onChange={(ends_at) => set({ ends_at })} />
            <span className="hint" style={{ alignSelf: 'end', paddingBottom: 8 }}>scheduled → running → terminated; players already inside keep going</span>
          </div>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(j) => <Runs j={j} />}
    />
  );
}
