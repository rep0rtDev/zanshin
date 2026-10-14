import { EntityPage, Text, Num, Tags, DateTime, CONDITION_HINT } from '../crud';
import { Badge, Field, JsonField, Toggle, fmtDate } from '../ui';

type LiveEvent = {
  key: string; name: string; description: string; enabled: boolean; condition: unknown; audiences: string[];
  starts_at: string; ends_at: string | null; duration_secs: number | null; recurrence: string | null; payload: unknown; tags: string[]; version: number; updated_at: string;
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
  return (
    <EntityPage<LiveEvent>
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
          <Field label="Targeting condition" hint={CONDITION_HINT}><JsonField value={d.condition ?? undefined} onChange={(condition) => set({ condition: condition ?? null })} rows={3} /></Field>
          <Text label="Audiences" value={(d.audiences ?? []).join(', ')} onChange={(v) => set({ audiences: v.split(',').map((s) => s.trim()).filter(Boolean) })} hint="audience keys, comma-separated" />
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
    />
  );
}
