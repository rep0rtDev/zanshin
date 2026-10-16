import { useState } from 'react';
import { get, post, useLoad } from '../api';
import { EntityPage, Num, Tags, Text } from '../crud';
import { Badge, ErrorBox, Field, JsonField, Table, Toggle, fmtDate, fmtNum } from '../ui';
import { t, tf } from '../i18n';

type Webhook = {
  id: string; key: string; name: string; url: string; secret: string; events: string[]; enabled: boolean; batch_size: number; batch_window_secs: number;
  headers: Record<string, unknown>; max_attempts: number; tags: string[]; version: number; updated_at: string;
};
type Delivery = { id: string; event_types: string[]; status: 'pending' | 'sent' | 'failed' | 'dead'; attempts: number; next_attempt_at: string | null; last_status: number | null; last_error: string; created_at: string; delivered_at: string | null };

const TONE: Record<Delivery['status'], 'ok' | 'bad' | 'warn' | 'info'> = { sent: 'ok', failed: 'warn', dead: 'bad', pending: 'info' };

function EventsPicker({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const q = useLoad(() => get<{ items: string[] }>('/webhooks/event-types'), []);
  const [custom, setCustom] = useState('');
  const toggle = (t: string) => onChange(value.includes(t) ? value.filter((x) => x !== t) : [...value.filter((x) => x !== '*'), t]);
  const groups = Array.from(new Set((q.data?.items ?? []).map((t) => t.split('.')[0])));
  return (
    <div className="stack">
      <div className="chips">
        <button type="button" className={'chip' + (value.includes('*') ? ' on' : '')} onClick={() => onChange(['*'])}>* everything</button>
        {groups.map((g) => { const p = `${g}.*`; return <button key={p} type="button" className={'chip' + (value.includes(p) ? ' on' : '')} onClick={() => toggle(p)}>{p}</button>; })}
      </div>
      <div className="chips">
        {(q.data?.items ?? []).filter((t) => !t.includes('<')).map((t) => <button key={t} type="button" className={'chip' + (value.includes(t) ? ' on' : '')} onClick={() => toggle(t)}>{t}</button>)}
      </div>
      <div className="row" style={{ alignItems: 'end' }}>
        <Text label={t('Custom pattern')} value={custom} onChange={setCustom} mono placeholder="event.purchase" hint="analytics events arrive as event.<name>" />
        <button className="btn ghost" disabled={!custom} onClick={() => { toggle(custom); setCustom(''); }}>{t('Add')}</button>
      </div>
      <div className="hint">Subscribed: {value.map((v) => <code key={v} style={{ marginRight: 6 }}>{v}</code>)}</div>
    </div>
  );
}

function Deliveries({ w }: { w: Webhook }) {
  const [status, setStatus] = useState('');
  const [err, setErr] = useState<Error | null>(null);
  const [test, setTest] = useState<Delivery | null>(null);
  const k = encodeURIComponent(w.key);
  const q = useLoad(() => get<{ items: Delivery[]; stats: { status: string; count: number }[] }>(`/webhooks/${k}/deliveries?limit=100${status ? '&status=' + status : ''}`), [w.key, w.version, status]);
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}>
        <strong>{t('Deliveries')}</strong>
        <select value={status} onChange={(e) => setStatus(e.target.value)}><option value="">all</option><option value="sent">sent</option><option value="failed">failed</option><option value="dead">dead</option><option value="pending">pending</option></select>
        <button className="btn ghost sm" onClick={q.reload}>{t('Refresh')}</button>
        <button className="btn sm" style={{ marginLeft: 'auto' }} onClick={() => post<Delivery>(`/webhooks/${k}/test`).then((d) => { setTest(d); setErr(null); q.reload(); }, setErr)}>{t('Send test event')}</button>
      </div>
      <ErrorBox error={err ?? q.error} />
      {test && <div style={{ marginTop: 8 }}><Badge tone={TONE[test.status]}>{test.status}</Badge> <span className="hint">{test.last_status ? tf('HTTP {0}', test.last_status) : ''} {test.last_error}</span></div>}
      <div className="chips" style={{ marginTop: 8 }}>{(q.data?.stats ?? []).map((s) => <span key={s.status} className="chip">{s.status}: {fmtNum(s.count)}</span>)}</div>
      <Table<Delivery>
        rows={q.data?.items}
        rowKey={(d) => d.id}
        empty={t('No deliveries yet')}
        cols={[
          { h: 'When', c: (d) => fmtDate(d.created_at) },
          { h: 'Events', c: (d) => d.event_types.map((t) => <code key={t} style={{ marginRight: 4 }}>{t}</code>) },
          { h: 'Status', c: (d) => <><Badge tone={TONE[d.status]}>{d.status}</Badge> <span className="hint">{d.attempts}× {d.last_status ? tf('HTTP {0}', d.last_status) : ''}</span></> },
          { h: 'Detail', c: (d) => <span className="hint">{d.last_error || (d.next_attempt_at && d.status === 'failed' ? tf('retry {0}', fmtDate(d.next_attempt_at)) : '')}</span> },
          { h: '', c: (d) => d.status !== 'sent' ? <button className="btn ghost sm" onClick={() => post(`/webhooks/${k}/deliveries/${d.id}/retry`).then(q.reload, setErr)}>{t('Retry')}</button> : null },
        ]}
      />
    </div>
  );
}

export default function Webhooks() {
  return (
    <EntityPage<Webhook>
      title={t('Webhooks')}
      path="/webhooks"
      blurb="HTTP callbacks for things that happen in Zanshin: config changes, player lifecycle, analytics events, message deliveries. Requests are signed (HMAC-SHA256 over timestamp.body), optionally batched, and retried with backoff; every attempt is logged."
      cols={[
        { h: 'Key', c: (w) => <><strong>{w.key}</strong><div className="hint">{w.url}</div></> },
        { h: 'Events', c: (w) => w.events.slice(0, 4).map((e) => <code key={e} style={{ marginRight: 4 }}>{e}</code>) },
        { h: 'Enabled', c: (w) => w.enabled ? <Badge tone="ok">on</Badge> : <Badge>off</Badge> },
        { h: 'Batch', c: (w) => w.batch_size > 1 ? `${w.batch_size} / ${w.batch_window_secs}s` : '—' },
        { h: 'Updated', c: (w) => fmtDate(w.updated_at) },
      ]}
      blank={() => ({ key: '', name: '', url: '', secret: '', events: ['*'], enabled: true, batch_size: 1, batch_window_secs: 0, headers: {}, max_attempts: 5, tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label={t('Key')} value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label={t('Name')} value={d.name} onChange={(name) => set({ name })} />
            <div className="field fixed"><span className="label">{t('Enabled')}</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <div className="row">
            <Text label={t('URL')} value={d.url} onChange={(url) => set({ url })} mono placeholder="https://example.com/zanshin" />
            <Text label={t('Secret')} value={d.secret} onChange={(secret) => set({ secret })} mono hint="HMAC key; masked after saving — leave the mask to keep it" />
          </div>
          <Field label={t('Events')}><EventsPicker value={d.events ?? ['*']} onChange={(events) => set({ events })} /></Field>
          <div className="row">
            <Num label={t('Batch size')} value={d.batch_size} onChange={(batch_size) => set({ batch_size: batch_size ?? 1 })} min={1} max={500} hint="1 = one request per event" />
            <Num label={t('Batch window (s)')} value={d.batch_window_secs} onChange={(batch_window_secs) => set({ batch_window_secs: batch_window_secs ?? 0 })} min={0} max={3600} hint={t('flush after this long even if the batch is not full')} />
            <Num label={t('Max attempts')} value={d.max_attempts} onChange={(max_attempts) => set({ max_attempts: max_attempts ?? 5 })} min={1} max={20} hint={t('backoff 10s, 20s, 40s … up to 1h')} />
          </div>
          <Field label={t('Extra headers (JSON)')}><JsonField value={d.headers} onChange={(headers) => set({ headers: (headers as Record<string, unknown>) ?? {} })} rows={2} /></Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(w) => <Deliveries w={w} />}
    />
  );
}
