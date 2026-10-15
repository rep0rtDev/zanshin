import { useState } from 'react';
import { get, post, useLoad } from '../api';
import { EntityPage, Text, Num, Tags, DateTime } from '../crud';
import { AudiencePicker, ConditionEditor } from '../RuleBuilder';
import type { Cond } from '../RuleBuilder';
import { Badge, ErrorBox, Field, JsonField, Stat, Table, Toggle, fmtDate, fmtNum } from '../ui';

type Schedule = { kind: 'once' } | { kind: 'cron'; cron: string; window_secs?: number | null } | { kind: 'live_event'; live_event: string; anchor?: 'start' | 'end'; offset_secs?: number; window_secs?: number | null };
type Campaign = {
  template_key?: string | null; schedule?: Schedule; channels?: string[];
  frequency_cap?: { max: number; per_secs: number } | null; quiet_hours?: { start_hour: number; end_hour: number; timezone_prop?: string | null } | null;
  variants?: Variant[]; push_options?: Record<string, unknown> | null;
  key: string; name: string; enabled: boolean; title: string; body: string; image_url: string; payload: unknown; condition: Cond | null; audiences: string[];
  percentage: number; starts_at: string | null; ends_at: string | null; ttl_secs: number | null; priority: number; push: boolean; tags: string[]; version: number; updated_at: string;
};
type Stats = { delivered: number; read: number; consumed: number };

const SAMPLE: Record<string, unknown> = { default: { name: 'Kai', platform: 'ios', country: 'DE', app_version: '1.4.2' }, custom: { level: 12, vip: true }, computed: { session_count: 37 } };
function render(tpl: string, props: Record<string, unknown> = SAMPLE): string {
  return (tpl ?? '').replace(/\{\{\s*([a-zA-Z0-9_.]+)\s*\}\}/g, (_, path: string) => {
    const v = path.split('.').reduce<unknown>((acc, k) => (acc && typeof acc === 'object' ? (acc as Record<string, unknown>)[k] : undefined), props);
    return v === undefined || v === null ? '' : String(v);
  });
}

function Preview({ title, body, image }: { title?: string; body?: string; image?: string }) {
  return (
    <div className="field">
      <span className="label">Preview <span className="hint">(sample identity: Kai, iOS, DE, level 12, vip)</span></span>
      <div className="preview">
        <div className="t">{render(title ?? '') || <span className="muted">title</span>}</div>
        <div>{render(body ?? '') || <span className="muted">body</span>}</div>
        {image && <img src={image} alt="" onError={(e) => ((e.target as HTMLImageElement).style.display = 'none')} />}
      </div>
    </div>
  );
}

function CampaignStats({ c }: { c: Campaign }) {
  const q = useLoad(() => get<Stats>(`/messages/${encodeURIComponent(c.key)}/stats`), [c.key, c.version]);
  const s = q.data;
  return (
    <div className="card">
      <div style={{ display: 'flex', justifyContent: 'space-between' }}><strong>Delivery</strong><button className="btn ghost sm" onClick={q.reload}>Refresh</button></div>
      <ErrorBox error={q.error} />
      <div className="grid c3" style={{ marginTop: 10 }}>
        <Stat label="Delivered" value={fmtNum(s?.delivered ?? 0)} />
        <Stat label="Read" value={fmtNum(s?.read ?? 0)} sub={s?.delivered ? `${((s.read / s.delivered) * 100).toFixed(1)}%` : undefined} />
        <Stat label="Consumed" value={fmtNum(s?.consumed ?? 0)} sub={s?.delivered ? `${((s.consumed / s.delivered) * 100).toFixed(1)}%` : undefined} />
      </div>
      <div className="hint" style={{ marginTop: 8 }}>Each identity receives a campaign at most once, on its next inbox fetch while the campaign is live and it matches targeting.{c.push && ' Push copies go out to registered devices on the next sweep (every minute).'}</div>
    </div>
  );
}

function TemplatePicker({ value, onChange }: { value: string | null; onChange: (v: string | null) => void }) {
  const q = useLoad(() => get<{ items: { key: string; name: string; format: string }[] }>('/templates'), []);
  return (
    <Field label="Template" hint="optional: render title/body/payload from the template in each player's locale">
      <select value={value ?? ''} onChange={(e) => onChange(e.target.value || null)}>
        <option value="">— inline title/body —</option>
        {(q.data?.items ?? []).map((t) => <option key={t.key} value={t.key}>{t.key}{t.name ? ` — ${t.name}` : ''} ({t.format})</option>)}
      </select>
    </Field>
  );
}

function ScheduleEditor({ value, onChange }: { value: Schedule; onChange: (v: Schedule) => void }) {
  const ev = useLoad(() => get<{ items: { key: string }[] } | { key: string }[]>('/live-events'), []);
  const events = Array.isArray(ev.data) ? ev.data : ev.data?.items ?? [];
  const kind = value.kind;
  return (
    <div className="row" style={{ alignItems: 'end' }}>
      <Field label="Kind">
        <select value={kind} onChange={(e) => { const k = e.target.value as Schedule['kind']; onChange(k === 'once' ? { kind: 'once' } : k === 'cron' ? { kind: 'cron', cron: '0 0 12 * * *' } : { kind: 'live_event', live_event: events[0]?.key ?? '', anchor: 'start', offset_secs: 0 }); }}>
          <option value="once">once</option><option value="cron">cron (UTC)</option><option value="live_event">relative to live event</option>
        </select>
      </Field>
      {value.kind === 'cron' && <>
        <Text label="Cron (sec min hour dom mon dow)" value={value.cron} onChange={(cron) => onChange({ ...value, cron })} mono />
        <Num label="Window (s)" value={value.window_secs ?? null} onChange={(window_secs) => onChange({ ...value, window_secs: window_secs ?? null })} hint="blank = 24h" />
      </>}
      {value.kind === 'live_event' && <>
        <Field label="Live event"><select value={value.live_event} onChange={(e) => onChange({ ...value, live_event: e.target.value })}>{events.map((e) => <option key={e.key} value={e.key}>{e.key}</option>)}</select></Field>
        <Field label="Anchor"><select value={value.anchor ?? 'start'} onChange={(e) => onChange({ ...value, anchor: e.target.value as 'start' | 'end' })}><option value="start">event start</option><option value="end">event end</option></select></Field>
        <Num label="Offset (s)" value={value.offset_secs ?? 0} onChange={(offset_secs) => onChange({ ...value, offset_secs: offset_secs ?? 0 })} hint="negative = before" />
      </>}
    </div>
  );
}

function ChannelsPicker({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const q = useLoad(() => get<{ items: { key: string; kind: string; enabled: boolean }[] }>('/integrations'), []);
  const items = q.data?.items ?? [];
  return (
    <div className="chips">
      <span className="chip on">inbox</span>
      {items.map((i) => { const on = value.includes(i.key); return <button key={i.key} type="button" className={'chip' + (on ? ' on' : '')} onClick={() => onChange(on ? value.filter((k) => k !== i.key) : [...value.filter((k) => k !== 'inbox'), i.key, 'inbox'])}>{i.key} <span className="hint">{i.kind}</span></button>; })}
      {items.length === 0 && <span className="hint">No integrations yet — add one under Integrations.</span>}
    </div>
  );
}

type Variant = { key: string; weight: number; title: string; body: string; image_url: string; template_key?: string | null };
type VariantStat = Variant & { sent: number; read: number; consumed: number; read_rate: number | null };

function Variants({ c }: { c: Campaign }) {
  const q = useLoad(() => get<{ total_sent: number; untagged: number; items: VariantStat[] }>(`/messages/${encodeURIComponent(c.key)}/variants`), [c.key]);
  if (!(c.variants?.length)) return null;
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}><strong>Copy variants (A/B)</strong><span className="hint">{q.data?.total_sent ?? 0} sent</span><button className="btn ghost sm" onClick={q.reload}>Refresh</button></div>
      <ErrorBox error={q.error} />
      <Table<VariantStat>
        rows={q.data?.items}
        rowKey={(v) => v.key}
        empty="No deliveries yet"
        cols={[
          { h: 'Variant', c: (v) => <code>{v.key}</code> },
          { h: 'Weight', c: (v) => v.weight },
          { h: 'Title', c: (v) => v.title || (v.template_key ? <span className="hint">template {v.template_key}</span> : <span className="muted">campaign default</span>) },
          { h: 'Sent', c: (v) => v.sent },
          { h: 'Read', c: (v) => v.read },
          { h: 'Read rate', c: (v) => (v.read_rate === null ? '—' : `${(v.read_rate * 100).toFixed(1)}%`) },
        ]}
      />
    </div>
  );
}

type DeliveryRow = { id: string; external_id: string; occurrence: string | null; channel: string; status: string; detail: string; at: string };

function Deliveries({ c }: { c: Campaign }) {
  const [status, setStatus] = useState('');
  const q = useLoad(() => get<{ items: DeliveryRow[]; stats: { channel: string; status: string; count: number }[] }>(`/messages/${encodeURIComponent(c.key)}/deliveries?limit=100${status ? '&status=' + status : ''}`), [c.key, status]);
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}>
        <strong>Delivery log</strong>
        <select value={status} onChange={(e) => setStatus(e.target.value)}><option value="">all</option><option value="sent">sent</option><option value="failed">failed</option><option value="skipped">skipped</option></select>
        <button className="btn ghost sm" onClick={q.reload}>Refresh</button>
      </div>
      <ErrorBox error={q.error} />
      <div className="chips" style={{ marginTop: 8 }}>{(q.data?.stats ?? []).map((s) => <span key={s.channel + s.status} className="chip"><code>{s.channel}</code> {s.status}: {fmtNum(s.count)}</span>)}</div>
      <Table<DeliveryRow>
        rows={q.data?.items}
        rowKey={(d) => d.id}
        empty="No deliveries yet"
        cols={[
          { h: 'When', c: (d) => fmtDate(d.at) },
          { h: 'Player', c: (d) => <span className="mono">{d.external_id}</span> },
          { h: 'Channel', c: (d) => <code>{d.channel}</code> },
          { h: 'Status', c: (d) => <Badge tone={d.status === 'sent' ? 'ok' : d.status === 'failed' ? 'bad' : 'warn'}>{d.status}</Badge> },
          { h: 'Detail', c: (d) => <span className="hint">{d.detail}{d.occurrence ? ` · occurrence ${fmtDate(d.occurrence)}` : ''}</span> },
        ]}
      />
    </div>
  );
}

export function DirectSend({ identityId, onSent }: { identityId: string; onSent?: () => void }) {
  const [m, setM] = useState({ title: '', body: '', image_url: '', payload: {} as unknown, priority: 0, push: false });
  const [res, setRes] = useState<string | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const send = () => post<{ id: string }>(`/identities/${identityId}/messages`, m).then((r) => { setRes(r.id); setErr(null); onSent?.(); }, setErr);
  return (
    <div className="card">
      <strong>Send a direct message</strong>
      <ErrorBox error={err} />
      <div className="row" style={{ marginTop: 10 }}>
        <Text label="Title" value={m.title} onChange={(title) => setM({ ...m, title })} />
        <Num label="Priority" value={m.priority} onChange={(priority) => setM({ ...m, priority: priority ?? 0 })} />
        <div className="field fixed"><span className="label">Push</span><Toggle value={m.push} onChange={(push) => setM({ ...m, push })} label={m.push ? 'yes' : 'no'} /></div>
      </div>
      <Field label="Body"><textarea rows={3} value={m.body} onChange={(e) => setM({ ...m, body: e.target.value })} style={{ fontFamily: 'inherit' }} /></Field>
      <div className="row">
        <Text label="Image URL" value={m.image_url} onChange={(image_url) => setM({ ...m, image_url })} />
        <Field label="Payload (JSON)"><JsonField value={m.payload} onChange={(payload) => setM({ ...m, payload: payload ?? {} })} rows={1} /></Field>
      </div>
      <button className="btn" disabled={!m.title} onClick={send}>Send</button>
      {res && <span className="muted" style={{ marginLeft: 10 }}>delivered · {res}</span>}
    </div>
  );
}

export default function Messages() {
  return (
    <EntityPage<Campaign>
      title="Messages"
      path="/messages"
      blurb={<>In-app message campaigns delivered to each matching identity's inbox exactly once. <code>{'{{default.name}}'}</code>-style placeholders in title/body are rendered per identity. Direct one-off messages are sent from an identity's page.</>}
      cols={[
        { h: 'Key', c: (c) => <><strong>{c.key}</strong><div className="hint">{c.title}</div></> },
        { h: 'Status', c: (c) => c.enabled ? <Badge tone="ok">enabled</Badge> : <Badge>disabled</Badge> },
        { h: 'Window', c: (c) => c.starts_at || c.ends_at ? `${fmtDate(c.starts_at)} → ${fmtDate(c.ends_at)}` : 'always' },
        { h: 'Reach', c: (c) => `${c.percentage}%${c.audiences.length ? ' · ' + c.audiences.join(', ') : ''}` },
        { h: 'Push', c: (c) => c.push ? <Badge tone="info">push</Badge> : <span className="muted">—</span> },
        { h: 'Priority', c: (c) => c.priority },
      ]}
      blank={() => ({ key: '', template_key: null, schedule: { kind: 'once' }, channels: ['inbox'], name: '', enabled: true, title: '', body: '', image_url: '', payload: {}, condition: null, audiences: [], percentage: 100, starts_at: null, ends_at: null, ttl_secs: null, priority: 0, push: false, tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} hint="internal" />
            <div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <TemplatePicker value={d.template_key ?? null} onChange={(template_key) => set({ template_key })} />
          <Text label="Title" value={d.title} onChange={(title) => set({ title })} placeholder="Welcome back, {{default.name}}!" hint={d.template_key ? 'fallback only — the template renders title/body' : undefined} />
          <Field label="Body"><textarea rows={3} value={d.body ?? ''} onChange={(e) => set({ body: e.target.value })} style={{ fontFamily: 'inherit' }} /></Field>
          <div className="row">
            <Text label="Image URL" value={d.image_url} onChange={(image_url) => set({ image_url })} />
            <Field label="Payload (JSON)" hint="deep link, reward id, …"><JsonField value={d.payload} onChange={(payload) => set({ payload: payload ?? {} })} rows={1} /></Field>
          </div>
          <Preview title={d.title} body={d.body} image={d.image_url} />
          <div className="row">
            <DateTime label="Starts" value={d.starts_at} onChange={(starts_at) => set({ starts_at })} />
            <DateTime label="Ends" value={d.ends_at} onChange={(ends_at) => set({ ends_at })} />
            <Num label="TTL (seconds)" value={d.ttl_secs} onChange={(ttl_secs) => set({ ttl_secs: ttl_secs ?? null })} hint="expires in the inbox after this" />
          </div>
          <div className="row">
            <Num label="Reach %" value={d.percentage} onChange={(percentage) => set({ percentage: percentage ?? 100 })} min={0} max={100} />
            <Num label="Priority" value={d.priority} onChange={(priority) => set({ priority: priority ?? 0 })} hint="higher first in inbox" />
            <div className="field fixed"><span className="label">Push notification</span><Toggle value={!!d.push} onChange={(push) => set({ push })} label={d.push ? 'also push' : 'inbox only'} /></div>
          </div>
          <Field label="Schedule" hint="once = on the first eligible inbox fetch; cron / live event = once per occurrence, also pushed proactively every minute"><ScheduleEditor value={d.schedule ?? { kind: 'once' }} onChange={(schedule) => set({ schedule })} /></Field>
          <Field label="Channels" hint="inbox is always the delivery record; integrations are sent in addition"><ChannelsPicker value={d.channels ?? ['inbox']} onChange={(channels) => set({ channels })} /></Field>
          <div className="row">
            <Num label="Frequency cap: max" value={d.frequency_cap?.max ?? null} onChange={(max) => set({ frequency_cap: max ? { max, per_secs: d.frequency_cap?.per_secs ?? 86400 } : null })} min={1} hint="deliveries of this campaign per player…" />
            <Num label="…per hours" value={d.frequency_cap ? d.frequency_cap.per_secs / 3600 : null} onChange={(h) => set({ frequency_cap: d.frequency_cap ? { ...d.frequency_cap, per_secs: Math.max(1, Math.round((h ?? 24) * 3600)) } : null })} min={1} />
            <Num label="Quiet hours from" value={d.quiet_hours?.start_hour ?? null} onChange={(start_hour) => set({ quiet_hours: start_hour === null || start_hour === undefined ? null : { start_hour, end_hour: d.quiet_hours?.end_hour ?? 8, timezone_prop: d.quiet_hours?.timezone_prop ?? 'default.timezone' } })} min={0} max={23} hint="local hour; blank = none" />
            <Num label="…to" value={d.quiet_hours?.end_hour ?? null} onChange={(end_hour) => set({ quiet_hours: d.quiet_hours ? { ...d.quiet_hours, end_hour: end_hour ?? 8 } : null })} min={0} max={23} />
            <Text label="Timezone property" value={d.quiet_hours?.timezone_prop ?? ''} onChange={(timezone_prop) => set({ quiet_hours: d.quiet_hours ? { ...d.quiet_hours, timezone_prop: timezone_prop || null } : null })} mono hint="default.timezone; UTC when the player has none" />
          </div>
          <Field label="Copy variants (A/B, JSON)" hint='[{"key":"a","weight":50,"title":"…","body":"…","image_url":"","template_key":null}] — sticky per player, stats on the campaign page'><JsonField value={d.variants ?? []} onChange={(variants) => set({ variants: (variants as Variant[] | null) ?? [] })} rows={2} /></Field>
          <Field label="Rich push options (JSON)" hint='{"sound":"default","badge":1,"channel_id":"promo","collapse_key":"daily","deep_link":"game://shop","actions":[{"id":"open","title":"Open","url":"game://shop"}]}'><JsonField value={d.push_options ?? null} onChange={(push_options) => set({ push_options: (push_options as Record<string, unknown> | null) ?? null })} rows={1} /></Field>
          <Field label="Targeting condition"><ConditionEditor optional value={d.condition} onChange={(condition) => set({ condition })} /></Field>
          <Field label="Audiences"><AudiencePicker value={d.audiences ?? []} onChange={(audiences) => set({ audiences })} /></Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(c) => <><CampaignStats c={c} /><Variants c={c} /><Deliveries c={c} /></>}
    />
  );
}
