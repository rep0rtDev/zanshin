import { get, useLoad } from '../api';
import { DateTime, EntityPage, Num, Tags, Text } from '../crud';
import { AudiencePicker, ConditionEditor } from '../RuleBuilder';
import type { Cond } from '../RuleBuilder';
import { Badge, ErrorBox, Field, JsonField, Table, Toggle, fmtDate } from '../ui';
import { Calendar } from '../Calendar';
import { useNavigate } from 'react-router-dom';

type LiveEvent = {
  key: string; name: string; description: string; enabled: boolean; condition: Cond | null; audiences: string[];
  starts_at: string; ends_at: string | null; duration_secs: number | null; recurrence: string | null; payload: unknown; join_mode?: 'auto' | 'explicit'; sticky?: boolean; participation_secs?: number | null; flag_overrides?: Record<string, unknown>; tags: string[]; version: number; updated_at: string;
};

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
      title="Live events"
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
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} />
            <div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <Text label="Description" value={d.description} onChange={(description) => set({ description })} />
          <div className="row">
            <DateTime label="Starts" value={d.starts_at} onChange={(v) => set({ starts_at: v ?? new Date().toISOString() })} hint="first occurrence (recurring) or window start" />
            <Num label="Duration (seconds)" value={d.duration_secs} onChange={(duration_secs) => set({ duration_secs: duration_secs ?? null })} min={1} hint="length of each occurrence" />
          </div>
          <div className="row">
            <Text label="Recurrence (cron, UTC)" value={d.recurrence ?? ''} onChange={(v) => set({ recurrence: v || null })} mono placeholder="0 0 18 * * Fri *" hint="7-field cron: sec min hour day month weekday year; empty = one-shot" />
            <DateTime label="Ends" value={d.ends_at} onChange={(ends_at) => set({ ends_at })} hint="one-shot: window end · recurring: no occurrences after this" />
          </div>
          <Field label="Payload (JSON)" hint="arbitrary data delivered to clients while the event is active"><JsonField value={d.payload} onChange={(payload) => set({ payload: payload ?? {} })} rows={5} /></Field>
          <div className="row">
            <Field label="Participation" hint="explicit: players call join during a window">
              <select value={d.join_mode ?? 'auto'} onChange={(e) => set({ join_mode: e.target.value as 'auto' | 'explicit' })}><option value="auto">auto (everyone targeted)</option><option value="explicit">explicit join</option></select>
            </Field>
            <Num label="Duration per player (sec)" value={d.participation_secs ?? null} onChange={(participation_secs) => set({ participation_secs: participation_secs ?? null })} min={1} hint="blank = until the window closes" />
            <div className="field fixed"><span className="label">Sticky</span><Toggle value={!!d.sticky} onChange={(sticky) => set({ sticky })} label={d.sticky ? 'keep participants across windows' : 'off'} /></div>
          </div>
          <Field label="Flag overrides (JSON)" hint='{"flag_key": value} applied to participants while active — above rules, below experiments'><JsonField value={d.flag_overrides ?? {}} onChange={(flag_overrides) => set({ flag_overrides: (flag_overrides ?? {}) as Record<string, unknown> })} rows={3} /></Field>
          <Field label="Targeting condition"><ConditionEditor optional value={d.condition} onChange={(condition) => set({ condition })} /></Field>
          <Field label="Audiences"><AudiencePicker value={d.audiences ?? []} onChange={(audiences) => set({ audiences })} /></Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(e) => <Participants ev={e} />}
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
          <strong>Timeline</strong>
          {t.data && <span className="hint">{t.data.participants.active} active · {t.data.participants.total} joined in total</span>}
        </div>
        <ErrorBox error={t.error} />
        <Table<{ start: string; end: string }>
          rows={t.data?.occurrences}
          rowKey={(o) => o.start}
          empty="No occurrences in the last 30 / next 60 days"
          cols={[
            { h: 'Window', c: (o) => <>{fmtDate(o.start)} → {fmtDate(o.end)}</> },
            { h: 'State', w: '110px', c: (o) => { const s = Date.parse(o.start), e = Date.parse(o.end); return s <= now && now < e ? <Badge tone="ok">live</Badge> : e <= now ? <Badge>past</Badge> : <Badge tone="info">upcoming</Badge>; } },
            { h: 'Joins that day', w: '130px', c: (o) => t.data?.joins.find((j) => j.day.slice(0, 10) === o.start.slice(0, 10))?.count ?? 0 },
          ]}
        />
      </div>
      <div className="card">
        <strong>Participants</strong>
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
