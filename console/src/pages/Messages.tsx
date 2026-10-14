import { useState } from 'react';
import { get, post, useLoad } from '../api';
import { EntityPage, Text, Num, Tags, DateTime } from '../crud';
import { AudiencePicker, ConditionEditor } from '../RuleBuilder';
import type { Cond } from '../RuleBuilder';
import { Badge, ErrorBox, Field, JsonField, Stat, Toggle, fmtDate, fmtNum } from '../ui';

type Campaign = {
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
      blank={() => ({ key: '', name: '', enabled: true, title: '', body: '', image_url: '', payload: {}, condition: null, audiences: [], percentage: 100, starts_at: null, ends_at: null, ttl_secs: null, priority: 0, push: false, tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} hint="internal" />
            <div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <Text label="Title" value={d.title} onChange={(title) => set({ title })} placeholder="Welcome back, {{default.name}}!" />
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
          <Field label="Targeting condition"><ConditionEditor optional value={d.condition} onChange={(condition) => set({ condition })} /></Field>
          <Field label="Audiences"><AudiencePicker value={d.audiences ?? []} onChange={(audiences) => set({ audiences })} /></Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(c) => <CampaignStats c={c} />}
    />
  );
}
