import { get, useLoad } from '../api';
import { DateTime, EntityPage, Num, Tags, Text } from '../crud';
import { AudiencePicker, ConditionEditor } from '../RuleBuilder';
import type { Cond } from '../RuleBuilder';
import { Badge, ErrorBox, Field, JsonField, Table, Toggle, fmtDate, fmtNum } from '../ui';
import { MetricsPicker } from './Experiments';
import { Calendar } from '../Calendar';
import { useNavigate } from 'react-router-dom';
import { t as tr } from '../i18n';

type LiveEvent = {
  key: string; name: string; description: string; enabled: boolean; condition: Cond | null; audiences: string[];
  starts_at: string; ends_at: string | null; duration_secs: number | null; recurrence: string | null; payload: unknown; join_mode?: 'auto' | 'explicit'; sticky?: boolean; participation_secs?: number | null; flag_overrides?: Record<string, unknown>; variants?: LEVariant[]; metrics?: string[]; tags: string[]; version: number; updated_at: string;
};
type LEVariant = { key: string; name: string; weight: number; payload: unknown; flag_overrides: Record<string, unknown> };

const now = () => Date.now();
function status(e: LiveEvent): ['ok' | 'muted' | 'info' | 'bad', string] {
  if (!e.enabled) return ['muted', 'disabled'];
  const s = new Date(e.starts_at).getTime();
  if (s > now()) return ['info', 'upcoming'];
  if (e.recurrence) return ['ok', 'recurring'];
  const end = e.duration_secs ? s + e.duration_secs * 1000 : e.ends_at ? new Date(e.ends_at).getTime() : null;
  if (end !== null && end < now()) return ['bad', 'ended'];
  return ['ok', 'live'];
}

export default function LiveEvents() {
  const nav = useNavigate();
  return (
    <EntityPage<LiveEvent>
      above={<Calendar onPick={(k) => nav(encodeURIComponent(k))} />}
      title={tr('Live events')}
      path="/live-events"
      blurb="Scheduled content windows — one-shot or recurring (cron, UTC) — with a payload the client reads when the event is active."
      sort={(a, b) => new Date(a.starts_at).getTime() - new Date(b.starts_at).getTime()}
      cols={[
        { h: 'Key', c: (e) => <><strong>{e.key}</strong><div className="hint">{e.name}</div></> },
        { h: 'Status', c: (e) => { const [t, l] = status(e); return <Badge tone={t}>{l}</Badge>; } },
        { h: 'Starts', c: (e) => fmtDate(e.starts_at) },
        { h: 'Schedule', c: (e) => e.recurrence ? <code>{e.recurrence}</code> : e.ends_at ? `until ${fmtDate(e.ends_at)}` : '—' },
        { h: 'Duration', c: (e) => e.duration_secs ? `${Math.round(e.duration_secs / 60)} min` : '—' },
      ]}
      blank={() => ({ key: '', name: '', description: '', enabled: true, condition: null, audiences: [], starts_at: new Date().toISOString(), ends_at: null, duration_secs: 3600, recurrence: null, payload: {}, tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label={tr('Key')} value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label={tr('Name')} value={d.name} onChange={(name) => set({ name })} />
            <div className="field fixed"><span className="label">{tr('Enabled')}</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <Text label={tr('Description')} value={d.description} onChange={(description) => set({ description })} />
          <div className="row">
            <DateTime label={tr('Starts')} value={d.starts_at} onChange={(v) => set({ starts_at: v ?? new Date().toISOString() })} hint={tr('first occurrence (recurring) or window start')} />
            <Num label={tr('Duration (seconds)')} value={d.duration_secs} onChange={(duration_secs) => set({ duration_secs: duration_secs ?? null })} min={1} hint={tr('length of each occurrence')} />
          </div>
          <div className="row">
            <Text label={tr('Recurrence (cron, UTC)')} value={d.recurrence ?? ''} onChange={(v) => set({ recurrence: v || null })} mono placeholder="0 0 18 * * Fri *" hint="7-field cron: sec min hour day month weekday year; empty = one-shot" />
            <DateTime label={tr('Ends')} value={d.ends_at} onChange={(ends_at) => set({ ends_at })} hint={tr('one-shot: window end · recurring: no occurrences after this')} />
          </div>
          <Field label={tr('Payload (JSON)')} hint={tr('arbitrary data delivered to clients while the event is active')}><JsonField value={d.payload} onChange={(payload) => set({ payload: payload ?? {} })} rows={5} /></Field>
          <div className="row">
            <Field label={tr('Participation')} hint={tr('explicit: players call join during a window')}>
              <select value={d.join_mode ?? 'auto'} onChange={(e) => set({ join_mode: e.target.value as 'auto' | 'explicit' })}><option value="auto">{tr('auto (everyone targeted)')}</option><option value="explicit">{tr('explicit join')}</option></select>
            </Field>
            <Num label={tr('Duration per player (sec)')} value={d.participation_secs ?? null} onChange={(participation_secs) => set({ participation_secs: participation_secs ?? null })} min={1} hint="blank = until the window closes" />
            <div className="field fixed"><span className="label">{tr('Sticky')}</span><Toggle value={!!d.sticky} onChange={(sticky) => set({ sticky })} label={d.sticky ? 'keep participants across windows' : 'off'} /></div>
          </div>
          <Field label={tr('Flag overrides (JSON)')} hint='{"flag_key": value} applied to participants while active — above rules, below experiments'><JsonField value={d.flag_overrides ?? {}} onChange={(flag_overrides) => set({ flag_overrides: (flag_overrides ?? {}) as Record<string, unknown> })} rows={3} /></Field>
          <Field label={tr('Variants')} hint="optional: each participant is deterministically assigned one by weight and gets its payload (blank = event payload) and flag overrides on top of the event's; metrics are reported per variant">
            <VariantsPanel value={d.variants ?? []} onChange={(variants) => set({ variants })} />
          </Field>
          <Field label={tr('Monitor metrics')} hint={tr('catalog metrics evaluated over participants, per run and per variant')}><MetricsPicker value={d.metrics ?? []} onChange={(metrics) => set({ metrics })} /></Field>
          <Field label={tr('Targeting condition')}><ConditionEditor optional value={d.condition} onChange={(condition) => set({ condition })} /></Field>
          <Field label={tr('Audiences')}><AudiencePicker value={d.audiences ?? []} onChange={(audiences) => set({ audiences })} /></Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(e) => <><Monitor ev={e} /><Participants ev={e} /></>}
    />
  );
}

type Participant = { identity_id: string; external_id: string; joined_at: string; left_at: string | null };
type Timeline = { occurrences: { start: string; end: string }[]; joins: { day: string; count: number }[]; participants: { total: number; active: number } };

function Participants({ ev }: { ev: LiveEvent }) {
  const k = encodeURIComponent(ev.key);
  const p = useLoad(() => get<{ items: Participant[] }>(`/live-events/${k}/participants?limit=200`), [ev.key]);
  const t = useLoad(() => get<Timeline>(`/live-events/${k}/timeline`), [ev.key, ev.version]);
  const now = Date.now();
  return (
    <>
      <div className="card">
        <div className="row" style={{ alignItems: 'center' }}>
          <strong>{tr('Timeline')}</strong>
          {t.data && <span className="hint">{t.data.participants.active} active · {t.data.participants.total} joined in total</span>}
        </div>
        <ErrorBox error={t.error} />
        <Table<{ start: string; end: string }>
          rows={t.data?.occurrences}
          rowKey={(o) => o.start}
          empty={tr('No occurrences in the last 30 / next 60 days')}
          cols={[
            { h: 'Window', c: (o) => <>{fmtDate(o.start)} → {fmtDate(o.end)}</> },
            { h: 'State', w: '110px', c: (o) => { const s = Date.parse(o.start), e = Date.parse(o.end); return s <= now && now < e ? <Badge tone="ok">live</Badge> : e <= now ? <Badge>past</Badge> : <Badge tone="info">upcoming</Badge>; } },
            { h: 'Joins that day', w: '130px', c: (o) => t.data?.joins.find((j) => j.day.slice(0, 10) === o.start.slice(0, 10))?.count ?? 0 },
          ]}
        />
      </div>
      <div className="card">
        <strong>{tr('Participants')}</strong>
        <ErrorBox error={p.error} />
        <Table<Participant>
          rows={p.data?.items}
          rowKey={(m) => m.identity_id}
          empty={ev.join_mode === 'explicit' ? 'Nobody has joined yet' : 'No explicit joins (auto events include every targeted player)'}
          cols={[
            { h: 'Player', c: (m) => <span className="mono">{m.external_id}</span> },
            { h: 'Joined', c: (m) => fmtDate(m.joined_at) },
            { h: 'Left', c: (m) => m.left_at ? fmtDate(m.left_at) : <Badge tone="ok">participating</Badge> },
          ]}
        />
      </div>
    </>
  );
}

function VariantsPanel({ value, onChange }: { value: LEVariant[]; onChange: (v: LEVariant[]) => void }) {
  const upd = (i: number, patch: Partial<LEVariant>) => onChange(value.map((v, j) => (j === i ? { ...v, ...patch } : v)));
  return (
    <div className="stack">
      {value.map((v, i) => (
        <div key={i} className="card" style={{ padding: 10 }}>
          <div className="row" style={{ alignItems: 'end' }}>
            <Text label={tr('Key')} value={v.key} onChange={(key) => upd(i, { key })} mono />
            <Text label={tr('Name')} value={v.name} onChange={(name) => upd(i, { name })} />
            <Num label={tr('Weight')} value={v.weight} onChange={(weight) => upd(i, { weight: weight ?? 1 })} min={0.01} />
            <button className="btn ghost sm" onClick={() => onChange(value.filter((_, j) => j !== i))}>{tr('Remove')}</button>
          </div>
          <div className="row">
            <Field label="Payload (JSON, blank = event payload)"><JsonField value={v.payload} onChange={(payload) => upd(i, { payload: payload ?? null })} rows={2} /></Field>
            <Field label={tr('Flag overrides (JSON)')}><JsonField value={v.flag_overrides} onChange={(fo) => upd(i, { flag_overrides: (fo ?? {}) as Record<string, unknown> })} rows={2} /></Field>
          </div>
        </div>
      ))}
      <div><button className="btn ghost" onClick={() => onChange([...value, { key: `v${value.length + 1}`, name: '', weight: 1, payload: null, flag_overrides: {} }])}>{tr('Add variant')}</button></div>
    </div>
  );
}

type MetricRow = { metric: string; name?: string; kind?: string; aggregation?: string; error?: string; variants?: { variant: string; value: number; count: number; identities: number }[] };
type Monitor = { variants: string[]; metrics: MetricRow[]; series: { metric: string; points: { variant: string; day: string; value: number }[] } | null };
type Runs = { runs: { run: number; start: string; end: string; live: boolean; metrics: MetricRow[] }[] };

function Monitor({ ev }: { ev: LiveEvent }) {
  const k = encodeURIComponent(ev.key);
  const m = useLoad(() => get<Monitor>(`/live-events/${k}/metrics`), [ev.key, ev.version]);
  const r = useLoad(() => get<Runs>(`/live-events/${k}/runs?limit=12`), [ev.key, ev.version]);
  if (!ev.metrics?.length) return <div className="card"><strong>{tr('Monitor metrics')}</strong><div className="hint">{tr('Attach catalog metrics to this event to see per-run and per-variant results here.')}</div></div>;
  const cols = m.data?.variants.length ? m.data.variants : ['default'];
  return (
    <>
      <div className="card">
        <strong>{tr('Monitor metrics')}</strong> <span className="hint">{tr('since the first occurrence, per variant')}</span>
        <ErrorBox error={m.error} />
        {m.data && (
          <table className="table" style={{ marginTop: 8 }}>
            <thead><tr><th>{tr('Metric')}</th>{cols.map((c) => <th key={c}>{c}</th>)}</tr></thead>
            <tbody>{m.data.metrics.map((x) => <tr key={x.metric}><td><strong>{x.metric}</strong><div className="hint">{x.error ?? `${x.kind} · ${x.aggregation}`}</div></td>{cols.map((c) => { const v = x.variants?.find((y) => y.variant === c); return <td key={c}>{v ? <>{fmtNum(v.value)} <span className="hint">({fmtNum(v.identities)} players)</span></> : '—'}</td>; })}</tr>)}</tbody>
          </table>
        )}
      </div>
      <div className="card">
        <strong>{tr('Run report')}</strong> <span className="hint">{tr('compare the same event across its runs')}</span>
        <ErrorBox error={r.error} />
        <Table<Runs['runs'][number]>
          rows={r.data?.runs}
          rowKey={(x) => x.start}
          empty={tr('No run has started yet')}
          cols={[
            { h: 'Run', w: '70px', c: (x) => <>#{x.run} {x.live && <Badge tone="ok">live</Badge>}</> },
            { h: 'Window', c: (x) => <>{fmtDate(x.start)} → {fmtDate(x.end)}</> },
            ...(ev.metrics ?? []).map((mk) => ({ h: mk, c: (x: Runs['runs'][number]) => { const row = x.metrics.find((y) => y.metric === mk); return row?.variants ? row.variants.map((v) => <div key={v.variant}><span className="hint">{v.variant}: </span>{fmtNum(v.value)}</div>) : <span className="muted">—</span>; } })),
          ]}
        />
      </div>
    </>
  );
}
