import { useState } from 'react';
import { get, post, useLoad } from '../api';
import { Text } from '../crud';
import { Badge, Confirm, ErrorBox, Field, Table, fmtDate } from '../ui';
import { t } from '../i18n';

type CR = { id: string; kind: string; key: string; body: unknown; title: string; description: string; status: string; author: string; approvals: { by: string; at: string; comment?: string }[]; required_approvals: number; error: string; created_at: string; applied_at?: string | null };

const KINDS = ['flags', 'audiences', 'experiments', 'live_events', 'messages', 'journeys', 'metrics', 'templates', 'integrations', 'webhooks', 'dashboards', 'export_sinks', 'event_definitions'];

export default function ChangeRequests() {
  const [status, setStatus] = useState('');
  const q = useLoad(() => get<{ items: CR[] }>(`/change-requests${status ? '?status=' + status : ''}`), [status]);
  const [err, setErr] = useState<Error | null>(null);
  const [d, setD] = useState({ kind: 'flags', key: '', title: '', description: '', body: '{\n  "enabled": false\n}', required_approvals: 1 });
  const act = (id: string, a: string, body?: unknown) => post(`/change-requests/${id}/${a}`, body).then(() => { setErr(null); q.reload(); }, setErr);
  const create = async () => {
    try {
      await post('/change-requests', { ...d, body: JSON.parse(d.body) });
      setErr(null); setD({ ...d, key: '', title: '', description: '' }); q.reload();
    } catch (e) { setErr(e as Error); }
  };
  const tone = (s: string) => s === 'applied' ? 'ok' : s === 'approved' ? 'info' : s === 'rejected' ? 'bad' : s === 'open' ? 'warn' : undefined;
  return (
    <div className="page">
      <div className="page-head"><h1>{t('Change requests')}</h1><p className="blurb">Propose a configuration change, get it approved by someone else (four-eyes), then apply it. Works for every config kind; the applied patch goes through the same path as config import, so it shows up in the audit log and history.</p></div>
      <ErrorBox error={err ?? q.error} />
      <div className="card">
        <div className="row" style={{ alignItems: 'center' }}>
          <strong>{t('Requests')}</strong>
          <select value={status} onChange={(e) => setStatus(e.target.value)}><option value="">all</option>{['open', 'approved', 'applied', 'rejected', 'cancelled'].map((s) => <option key={s} value={s}>{s}</option>)}</select>
        </div>
        <Table<CR> rows={q.data?.items ?? []} rowKey={(r) => r.id} empty={t('No change requests.')}
          cols={[
            { h: 'Change', c: (r) => <><strong>{r.title || `${r.kind}/${r.key}`}</strong><div className="hint"><code>{r.kind}</code> · <code>{r.key}</code> · by {r.author} · {fmtDate(r.created_at)}</div>{r.description && <div className="hint">{r.description}</div>}</> },
            { h: 'Patch', c: (r) => <code style={{ whiteSpace: 'pre-wrap' }}>{JSON.stringify(r.body)}</code> },
            { h: 'Status', c: (r) => <><Badge tone={tone(r.status)}>{r.status}</Badge><div className="hint">{r.approvals.length}/{r.required_approvals} approvals{r.approvals.map((a) => <div key={a.by}>{a.by} · {fmtDate(a.at)}{a.comment ? ` — ${a.comment}` : ''}</div>)}{r.error && <div>{r.error}</div>}</div></> },
            { h: '', c: (r) => (
              <div className="actions">
                {r.status === 'open' && <button className="btn sm" onClick={() => act(r.id, 'approve')}>{t('Approve')}</button>}
                {r.status === 'approved' && <Confirm onConfirm={() => act(r.id, 'apply')}>{t('Apply')}</Confirm>}
                {(r.status === 'open' || r.status === 'approved') && <button className="btn ghost sm" onClick={() => act(r.id, 'reject')}>{t('Reject')}</button>}
                {(r.status === 'open' || r.status === 'approved') && <button className="btn ghost sm" onClick={() => act(r.id, 'cancel')}>{t('Cancel')}</button>}
              </div>
            ) },
          ]} />
      </div>
      <div className="card">
        <strong>{t('New change request')}</strong>
        <div className="row" style={{ alignItems: 'end', marginTop: 8 }}>
          <label className="field"><span className="label">{t('Kind')}</span><select value={d.kind} onChange={(e) => setD({ ...d, kind: e.target.value })}>{KINDS.map((k) => <option key={k} value={k}>{k}</option>)}</select></label>
          <Text label={t('Key')} value={d.key} onChange={(key) => setD({ ...d, key })} mono />
          <Text label={t('Title')} value={d.title} onChange={(title) => setD({ ...d, title })} />
          <label className="field fixed"><span className="label">{t('Approvals needed')}</span><input type="number" min={1} max={10} value={d.required_approvals} onChange={(e) => setD({ ...d, required_approvals: Number(e.target.value) || 1 })} /></label>
        </div>
        <Text label={t('Description')} value={d.description} onChange={(description) => setD({ ...d, description })} />
        <Field label={t('Patch (JSON, fields of the entity)')}><textarea className="mono" rows={4} value={d.body} onChange={(e) => setD({ ...d, body: e.target.value })} /></Field>
        <div style={{ marginTop: 8 }}><button className="btn" disabled={!d.key} onClick={create}>{t('Create request')}</button></div>
      </div>
    </div>
  );
}
