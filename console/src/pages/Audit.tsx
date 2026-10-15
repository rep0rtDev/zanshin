import { useState } from 'react';
import { get, qs, useLoad, post } from '../api';
import { Badge, ErrorBox, Page, Table, fmtDate } from '../ui';
import { t } from '../i18n';

type Entry = {
  id: string; at: string; actor_key_id: string | null; actor_name: string; actor_kind: string; method: string; path: string;
  status: number; request_id: string | null; ip: string | null; body: unknown;
};
const tone = (s: number) => (s < 300 ? 'ok' : s === 401 || s === 403 ? 'bad' : s < 500 ? 'warn' : 'bad');

export default function Audit() {
  const [f, setF] = useState({ actor: '', method: '', path: '' });
  const [applied, setApplied] = useState(f);
  const [after, setAfter] = useState<string | null>(null);
  const [stack, setStack] = useState<string[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  const q = useLoad(() => get<{ items: Entry[]; next: string | null }>('/audit' + qs({ ...applied, after, limit: 50 })), [applied, after]);
  const apply = (e: React.FormEvent) => { e.preventDefault(); setStack([]); setAfter(null); setApplied(f); };
  return (
    <Page title={t('Audit log')}>
      <p className="muted" style={{ marginTop: -8 }}>{t('Every mutating call to the admin API, including rejected attempts. Request bodies are stored with secrets redacted.')}</p>
      <form className="card row" onSubmit={apply}>
        <label className="field"><span className="label">{t('Actor')}</span><input value={f.actor} onChange={(e) => setF({ ...f, actor: e.target.value })} placeholder="bootstrap" /></label>
        <label className="field"><span className="label">{t('Method')}</span>
          <select value={f.method} onChange={(e) => setF({ ...f, method: e.target.value })}><option value="">any</option>{['POST', 'PUT', 'PATCH', 'DELETE'].map((m) => <option key={m}>{m}</option>)}</select>
        </label>
        <label className="field"><span className="label">{t('Path prefix')}</span><input className="mono" value={f.path} onChange={(e) => setF({ ...f, path: e.target.value })} placeholder="/admin/v1/flags" /></label>
        <div className="field fixed"><span className="label">&nbsp;</span><button className="btn">{t('Filter')}</button></div>
      </form>
      <ErrorBox error={q.error} />
      <Table<Entry>
        rows={q.data?.items}
        rowKey={(e) => e.id}
        onRow={(e) => setOpen(open === e.id ? null : e.id)}
        empty={t('No admin mutations recorded yet')}
        cols={[
          { h: 'When', w: '190px', c: (e) => <span style={{ whiteSpace: 'nowrap' }}>{fmtDate(e.at)}</span> },
          { h: 'Actor', c: (e) => <><strong>{e.actor_name}</strong> <Badge tone={e.actor_kind === 'admin' ? 'info' : 'muted'}>{e.actor_kind}</Badge></> },
          { h: 'Call', c: (e) => <span className="mono"><Badge>{e.method}</Badge> {e.path}</span> },
          { h: 'Status', w: '80px', c: (e) => <Badge tone={tone(e.status)}>{e.status}</Badge> },
          { h: '', w: '70px', c: (e) => (e.status < 300 && e.method !== 'GET' && /^\/admin\/v1\/(flags|audiences|experiments|live-events|messages|webhooks|journeys|dashboards|templates|integrations|metrics|export-sinks)/.test(e.path) ? <button className="btn ghost sm" onClick={() => post(`/audit/${e.id}/undo`, {}).then(() => alert('Reverted'), (err: Error) => alert(err.message))}>{t('Undo')}</button> : null) },
          { h: 'IP', w: '130px', c: (e) => <span className="mono hint">{e.ip ?? '—'}</span> },
          { h: 'Body', c: (e) => e.body == null ? <span className="muted">—</span> : open === e.id ? <pre className="code">{JSON.stringify(e.body, null, 2)}</pre> : <code className="hint trunc" title={t('click row to expand')}>{JSON.stringify(e.body)}</code> },
        ]}
      />
      <div className="actions" style={{ marginTop: 12 }}>
        <button className="btn ghost" disabled={!stack.length} onClick={() => { const s = [...stack]; const prev = s.pop(); setStack(s); setAfter(prev || null); }}>{t('Newer')}</button>
        <button className="btn ghost" disabled={!q.data?.next} onClick={() => { setStack([...stack, after ?? '']); setAfter(q.data!.next); }}>{t('Older')}</button>
      </div>
    </Page>
  );
}
