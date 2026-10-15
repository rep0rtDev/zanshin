import { useState } from 'react';
import { Route, Routes, useNavigate, useParams, Link } from 'react-router-dom';
import { del, get, post, put, qs, useLoad } from '../api';
import { Badge, Confirm, ErrorBox, Field, JsonField, Page, Table, fmtDate } from '../ui';
import { DirectSend } from './Messages';

type Identity = { id: string; external_id: string; created_at: string; updated_at: string; last_seen_at: string; properties: { default: Record<string, unknown>; custom: Record<string, unknown>; computed: Record<string, unknown> } };
type Ev = { id: string; event_time: string; name: string; value?: unknown; metadata?: unknown; experiments?: Record<string, string> };
type Tok = { platform: string; token: string; device_id: string; updated_at: string; failures: number };
type Inbox = { id: string; key: string | null; title: string; delivered_at: string; read_at: string | null; expires_at: string | null };

export default function Identities() {
  return (
    <Routes>
      <Route index element={<List />} />
      <Route path=":id" element={<Detail />} />
    </Routes>
  );
}

function List() {
  const nav = useNavigate();
  const [after, setAfter] = useState<string | null>(null);
  const [stack, setStack] = useState<string[]>([]);
  const [search, setSearch] = useState('');
  const [err, setErr] = useState<Error | null>(null);
  const q = useLoad(() => get<{ items: Identity[]; next: string | null }>('/identities' + qs({ after, limit: 50 })), [after]);
  const find = async (e: React.FormEvent) => {
    e.preventDefault();
    try { const id = await get<Identity>(`/identities/by-external-id/${encodeURIComponent(search.trim())}`); nav(id.id); } catch (e) { setErr(e as Error); }
  };
  return (
    <Page title="Identities" actions={<form onSubmit={find} className="inline"><input value={search} onChange={(e) => setSearch(e.target.value)} placeholder="find by external id" style={{ width: 260 }} /><button className="btn ghost">Open</button></form>}>
      <ErrorBox error={err ?? q.error} />
      <Table<Identity>
        rows={q.data?.items}
        rowKey={(i) => i.id}
        onRow={(i) => nav(i.id)}
        cols={[
          { h: 'External id', c: (i) => <strong className="mono">{i.external_id}</strong> },
          { h: 'Id', c: (i) => <span className="mono muted">{i.id}</span> },
          { h: 'Properties', c: (i) => <span className="hint">{Object.keys(i.properties.default).length} default · {Object.keys(i.properties.custom).length} custom · {Object.keys(i.properties.computed).length} computed</span> },
          { h: 'First seen', c: (i) => fmtDate(i.created_at) },
          { h: 'Last seen', c: (i) => fmtDate(i.last_seen_at) },
        ]}
      />
      <div className="actions" style={{ marginTop: 12 }}>
        <button className="btn ghost" disabled={!stack.length} onClick={() => { const s = [...stack]; const prev = s.pop(); setStack(s); setAfter(prev ?? null); }}>Previous</button>
        <button className="btn ghost" disabled={!q.data?.next} onClick={() => { setStack([...stack, after ?? '']); setAfter(q.data!.next); }}>Next</button>
      </div>
    </Page>
  );
}

function QaTime({ id }: { id: string }) {
  const q = useLoad(() => get<{ qa_time: string | null }>(`/identities/${id}/qa-time`), [id]);
  const [at, setAt] = useState('');
  const [err, setErr] = useState<Error | null>(null);
  const save = () => put(`/identities/${id}/qa-time`, { at: new Date(at).toISOString() }).then(() => { setErr(null); q.reload(); }, setErr);
  const clear = () => del(`/identities/${id}/qa-time`).then(() => { setErr(null); q.reload(); }, setErr);
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'end' }}>
        <div className="grow">
          <strong>QA time travel</strong>
          <div className="hint">Pin this player's clock to test future live events and schedules. {q.data?.qa_time ? <Badge tone="info">pinned to {fmtDate(q.data.qa_time)}</Badge> : <span className="muted">real time</span>}</div>
        </div>
        <Field label="Travel to"><input type="datetime-local" value={at} onChange={(e) => setAt(e.target.value)} /></Field>
        <button className="btn ghost" disabled={!at} onClick={save}>Pin</button>
        {q.data?.qa_time && <button className="btn ghost" onClick={clear}>Reset</button>}
      </div>
      <ErrorBox error={err} />
    </div>
  );
}

function Detail() {
  const { id } = useParams();
  const nav = useNavigate();
  const [tab, setTab] = useState<'props' | 'flags' | 'events' | 'inbox' | 'push' | 'journeys'>('props');
  const [err, setErr] = useState<Error | null>(null);
  const q = useLoad(() => get<Identity>(`/identities/${id}`), [id]);
  const i = q.data;
  return (
    <Page
      title={<span><a href="#" className="muted" onClick={(e) => { e.preventDefault(); nav('..', { relative: 'path' }); }}>Identities</a> / {i?.external_id ?? '…'}</span>}
      actions={<Confirm danger onConfirm={() => del(`/identities/${id}`).then(() => nav('..', { relative: 'path' }), setErr)}>Delete identity</Confirm>}
    >
      <ErrorBox error={err ?? q.error} />
      {i && (
        <>
          <div className="card">
            <dl className="kv">
              <dt>Id</dt><dd className="mono">{i.id}</dd>
              <dt>External id</dt><dd className="mono">{i.external_id}</dd>
              <dt>First seen</dt><dd>{fmtDate(i.created_at)}</dd>
              <dt>Last seen</dt><dd>{fmtDate(i.last_seen_at)}</dd>
            </dl>
          </div>
          <QaTime id={i.id} />
          <div className="tabs">
            {(['props', 'flags', 'events', 'inbox', 'push', 'journeys'] as const).map((t) => <button key={t} className={tab === t ? 'active' : ''} onClick={() => setTab(t)}>{{ props: 'Properties', flags: 'Flags', events: 'Events', inbox: 'Inbox', push: 'Push devices', journeys: 'Journeys' }[t]}</button>)}
          </div>
          {tab === 'props' && <Props i={i} onSaved={(n) => q.setData(n)} />}
          {tab === 'flags' && <FlagsTab id={i.id} />}
          {tab === 'events' && <Events id={i.id} />}
          {tab === 'inbox' && <InboxTab id={i.id} />}
          {tab === 'push' && <PushTab id={i.id} />}
          {tab === 'journeys' && <JourneysTab id={i.id} />}
        </>
      )}
    </Page>
  );
}

function Props({ i, onSaved }: { i: Identity; onSaved: (n: Identity) => void }) {
  const [dflt, setDflt] = useState<unknown>(i.properties.default);
  const [custom, setCustom] = useState<unknown>(i.properties.custom);
  const [err, setErr] = useState<Error | null>(null);
  const [ok, setOk] = useState(false);
  const save = () => put<Identity>(`/identities/${i.id}/properties`, { default: dflt, custom }).then((n) => { onSaved(n); setOk(true); setErr(null); }, setErr);
  return (
    <div className="grid c2">
      <div className="card">
        <ErrorBox error={err} />
        <Field label="Default properties" hint="set by the client at authenticate (platform, country, app version, …); merged on save"><JsonField value={dflt} onChange={setDflt} rows={8} /></Field>
        <Field label="Custom properties" hint="set by the game/backend (level, vip, …); merged on save, null deletes a key"><JsonField value={custom} onChange={setCustom} rows={8} /></Field>
        <button className="btn" onClick={save}>Save</button>{ok && <span className="muted" style={{ marginLeft: 10 }}>Saved</span>}
      </div>
      <div className="card">
        <strong>Computed properties</strong>
        <div className="hint" style={{ marginBottom: 8 }}>Derived from events by the server; read-only, usable in targeting as <code>computed.*</code>.</div>
        <pre className="code">{JSON.stringify(i.properties.computed, null, 2)}</pre>
      </div>
    </div>
  );
}

function Events({ id }: { id: string }) {
  const q = useLoad(() => get<{ events: Ev[] }>(`/identities/${id}/events?limit=200`), [id]);
  return (
    <>
      <ErrorBox error={q.error} />
      <Table<Ev>
        rows={q.data?.events}
        rowKey={(e) => e.id}
        empty="No events recorded"
        cols={[
          { h: 'Time', w: '180px', c: (e) => fmtDate(e.event_time) },
          { h: 'Name', c: (e) => <code>{e.name}</code> },
          { h: 'Value', c: (e) => e.value === undefined || e.value === null ? <span className="muted">—</span> : <code>{JSON.stringify(e.value)}</code> },
          { h: 'Metadata', c: (e) => e.metadata && Object.keys(e.metadata as object).length ? <code className="hint">{JSON.stringify(e.metadata)}</code> : <span className="muted">—</span> },
          { h: 'Experiments', c: (e) => e.experiments && Object.keys(e.experiments).length ? Object.entries(e.experiments).map(([k, v]) => <Badge key={k} tone="info">{k}={v}</Badge>) : <span className="muted">—</span> },
        ]}
      />
    </>
  );
}

type JRun = { id: string; step: number; status: 'active' | 'done' | 'exited' | 'failed'; wake_at: string | null; error: string; entered_at: string; updated_at: string; context: Record<string, unknown> };
type JLog = { step: number; action: string; detail: string; at: string };
const JTONE: Record<JRun['status'], 'ok' | 'bad' | 'warn' | 'info'> = { done: 'ok', failed: 'bad', exited: 'warn', active: 'info' };

type Resolved = { key: string; value: unknown; value_type: string; variant?: string; version: number; reason: { kind: string; rule_id?: string; variant?: string; experiment?: string; live_event?: string } };

const SOURCE_TONE: Record<string, 'ok' | 'info' | 'warn' | 'muted' | 'bad'> = { default: 'muted', disabled: 'muted', rule: 'info', variant: 'info', override: 'warn', experiment: 'ok', live_event: 'ok' };

/** Every flag as this player resolves it right now, with the source of the value (Satori "Player View → Feature Flags"). */
function FlagsTab({ id }: { id: string }) {
  const q = useLoad(() => get<{ items: Resolved[] }>(`/identities/${id}/flags`), [id]);
  const [filter, setFilter] = useState('');
  const [changedOnly, setChangedOnly] = useState(false);
  const rows = (q.data?.items ?? []).filter((r) => (!filter || r.key.includes(filter)) && (!changedOnly || !['default', 'disabled'].includes(r.reason.kind)));
  const source = (r: Resolved) => {
    const k = r.reason.kind;
    if (k === 'experiment') return `experiment ${r.reason.experiment} · ${r.reason.variant}`;
    if (k === 'live_event') return `live event ${r.reason.live_event}`;
    if (k === 'variant') return `rule ${r.reason.rule_id} · variant ${r.reason.variant}`;
    if (k === 'rule') return `rule ${r.reason.rule_id}`;
    if (k === 'override') return 'identity override';
    return k;
  };
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center', marginBottom: 8 }}>
        <input placeholder="filter by key" value={filter} onChange={(e) => setFilter(e.target.value)} style={{ maxWidth: 240 }} />
        <label className="hint" style={{ display: 'flex', gap: 6, alignItems: 'center' }}><input type="checkbox" checked={changedOnly} onChange={(e) => setChangedOnly(e.target.checked)} /> only values changed from default</label>
        <div className="spacer" />
        <span className="hint">{rows.length} of {q.data?.items.length ?? 0} flags</span>
      </div>
      <ErrorBox error={q.error} />
      {q.data && rows.length === 0 && <div className="empty">No flags match.</div>}
      {rows.length > 0 && (
        <table className="table">
          <thead><tr><th>Flag</th><th>Value</th><th>Source</th><th style={{ width: 60 }}>v</th><th /></tr></thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.key}>
                <td><Link to={`/flags/${encodeURIComponent(r.key)}`} className="mono">{r.key}</Link></td>
                <td className="mono" style={{ maxWidth: 320, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={JSON.stringify(r.value)}>{JSON.stringify(r.value)}</td>
                <td><Badge tone={SOURCE_TONE[r.reason.kind] ?? 'muted'}>{source(r)}</Badge></td>
                <td className="hint">{r.version}</td>
                <td><Link to={`/flags/${encodeURIComponent(r.key)}?explain=${id}`} className="hint">explain</Link></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function JourneysTab({ id }: { id: string }) {
  const q = useLoad(() => get<{ items: { journey: string; run: JRun; log: JLog[] }[] }>(`/identities/${id}/journeys`), [id]);
  const items = q.data?.items ?? [];
  return (
    <>
      <ErrorBox error={q.error} />
      {!q.data ? null : items.length === 0 ? <div className="empty">This player has not entered any journey.</div> : items.map(({ journey, run, log }) => (
        <div key={run.id} className="card">
          <div className="row" style={{ alignItems: 'center' }}>
            <Link to={`/journeys/${encodeURIComponent(journey)}`}><strong>{journey}</strong></Link>
            <Badge tone={JTONE[run.status]}>{run.status}</Badge>
            <span className="hint">entered {fmtDate(run.entered_at)}{run.context?.parent ? ` · from journey ${String(run.context.parent)}` : ''}{run.status === 'active' && run.wake_at ? ` · wakes ${fmtDate(run.wake_at)}` : ''}{run.error ? ` · ${run.error}` : ''}</span>
          </div>
          <table className="table" style={{ marginTop: 8 }}>
            <thead><tr><th style={{ width: 180 }}>Time</th><th style={{ width: 160 }}>Step</th><th>What happened</th></tr></thead>
            <tbody>{log.map((l, i) => <tr key={i}><td>{fmtDate(l.at)}</td><td><code>{l.step + 1}. {l.action}</code></td><td className="hint">{l.detail}</td></tr>)}</tbody>
          </table>
        </div>
      ))}
    </>
  );
}

function InboxTab({ id }: { id: string }) {
  const q = useLoad(() => get<{ messages: Inbox[] } | Inbox[]>(`/identities/${id}/messages`), [id]);
  const rows = Array.isArray(q.data) ? q.data : q.data?.messages;
  return (
    <>
      <ErrorBox error={q.error} />
      <DirectSend identityId={id} onSent={q.reload} />
      <Table<Inbox>
        rows={rows}
        rowKey={(m) => m.id}
        empty="Inbox is empty"
        cols={[
          { h: 'Delivered', w: '180px', c: (m) => fmtDate(m.delivered_at) },
          { h: 'Title', c: (m) => <><strong>{m.title}</strong>{m.key && <div className="hint">campaign {m.key}</div>}</> },
          { h: 'State', c: (m) => m.read_at ? <Badge>read</Badge> : <Badge tone="info">unread</Badge> },
          { h: 'Expires', c: (m) => fmtDate(m.expires_at) },
          { h: '', c: (m) => <button className="btn ghost sm" onClick={() => del(`/identities/${id}/inbox/${m.id}`).then(q.reload)}>Delete</button> },
        ]}
      />
    </>
  );
}

function PushTab({ id }: { id: string }) {
  const q = useLoad(() => get<{ tokens: Tok[] }>(`/identities/${id}/push-tokens`), [id]);
  const [err, setErr] = useState<Error | null>(null);
  return (
    <>
      <ErrorBox error={q.error ?? err} />
      <div className="actions" style={{ marginBottom: 10 }}>
        <button className="btn ghost sm" onClick={() => post('/push/sweep').then(() => setErr(null), setErr)}>Run push sweep now</button>
      </div>
      <Table<Tok>
        rows={q.data?.tokens}
        rowKey={(t) => t.token}
        empty="No registered devices"
        cols={[
          { h: 'Platform', c: (t) => <Badge tone="info">{t.platform}</Badge> },
          { h: 'Device', c: (t) => <span className="mono">{t.device_id || '—'}</span> },
          { h: 'Token', c: (t) => <span className="mono hint">{t.token.slice(0, 24)}…</span> },
          { h: 'Updated', c: (t) => fmtDate(t.updated_at) },
          { h: 'Failures', c: (t) => t.failures ? <Badge tone="warn">{t.failures}</Badge> : 0 },
        ]}
      />
    </>
  );
}
