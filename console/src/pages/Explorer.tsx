import { useState } from 'react';
import { del, get, post, put, useLoad } from '../api';
import { Num, Text } from '../crud';
import { Badge, ErrorBox, Field, Page, Table, Toggle, fmtDate, fmtNum } from '../ui';

type SqlResult = { columns: string[]; types: string[]; rows: unknown[][]; rows_read: number; elapsed_secs: number; truncated: boolean };
type SavedQuery = { key: string; name: string; description: string; sql: string; tags: string[]; updated_at: string };
type Report = { key: string; name: string; enabled: boolean; query_key: string; cron: string; timezone: string; channels: string[]; email: { integration: string; to: string[] } | null; max_rows: number; last_run_at: string | null; last_status: string };
type Alert = { key: string; name: string; enabled: boolean; event: string; aggregation: 'count' | 'unique' | 'sum'; field: string; condition: { kind: 'above' | 'below'; threshold: number } | { kind: 'anomaly'; zscore: number; baseline_windows: number; min_delta: number }; window_secs: number; cooldown_secs: number; last_value: number | null; last_checked_at: string | null; last_fired_at: string | null };
type Fire = { id: string; value: number; baseline: number | null; deviation: number | null; text: string; fired_at: string };
type Paths = { event?: string; anchors: number; truncated: boolean; levels: { depth: number; items: { name: string; count: number; share: number }[] }[]; sequences: { steps: string[]; count: number }[] };

const TABS = ['sql', 'queries', 'paths', 'alerts', 'reports'] as const;
const LABEL: Record<(typeof TABS)[number], string> = { sql: 'SQL', queries: 'Saved queries', paths: 'Paths', alerts: 'Alerts', reports: 'Reports' };

export default function Explorer() {
  const [tab, setTab] = useState<(typeof TABS)[number]>('sql');
  return (
    <Page title="Explorer">
      <div className="tabs">{TABS.map((t) => <button key={t} className={tab === t ? 'active' : ''} onClick={() => setTab(t)}>{LABEL[t]}</button>)}</div>
      {tab === 'sql' && <Sql />}
      {tab === 'queries' && <Queries />}
      {tab === 'paths' && <PathsView />}
      {tab === 'alerts' && <Alerts />}
      {tab === 'reports' && <Reports />}
    </Page>
  );
}

function ResultTable({ r }: { r: SqlResult }) {
  return (
    <div className="card">
      <div className="hint" style={{ marginBottom: 6 }}>{fmtNum(r.rows.length)} rows{r.truncated ? ' (truncated)' : ''} · {fmtNum(r.rows_read)} read · {r.elapsed_secs.toFixed(3)}s</div>
      <div style={{ overflowX: 'auto' }}>
        <table className="table">
          <thead><tr>{r.columns.map((c, i) => <th key={i}>{c}<div className="hint">{r.types[i]}</div></th>)}</tr></thead>
          <tbody>{r.rows.map((row, i) => <tr key={i}>{row.map((v, j) => <td key={j} className="mono">{v === null ? <span className="muted">null</span> : typeof v === 'object' ? JSON.stringify(v) : String(v)}</td>)}</tr>)}</tbody>
        </table>
      </div>
    </div>
  );
}

function Sql() {
  const st = useLoad(() => get<{ available: boolean; max_rows: number }>('/sql'), []);
  const [sql, setSql] = useState('SELECT name, count() AS events, uniqExact(identity_id) AS players\nFROM events\nWHERE event_time > now() - INTERVAL 7 DAY\nGROUP BY name ORDER BY events DESC\nLIMIT 50');
  const [limit, setLimit] = useState(1000);
  const [res, setRes] = useState<SqlResult | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const [busy, setBusy] = useState(false);
  const [saveKey, setSaveKey] = useState('');
  const [cohort, setCohort] = useState('');
  const [msg, setMsg] = useState('');
  const run = () => { setBusy(true); post<SqlResult>('/sql', { sql, limit }).then((d) => { setRes(d); setErr(null); }, setErr).finally(() => setBusy(false)); };
  const save = () => post('/analytics/queries', { key: saveKey, name: saveKey, sql }).then(() => { setMsg(`Saved as ${saveKey}`); setErr(null); }, setErr);
  const toCohort = () => post<{ audience: string; matched: number; members: number }>('/sql/cohort', { sql, audience_key: cohort }).then((d) => { setMsg(`${d.matched} players → audience ${d.audience} (${d.members} members)`); setErr(null); }, setErr);
  return (
    <div className="stack">
      {st.data && !st.data.available && <div className="card"><Badge tone="warn">ClickHouse is not enabled</Badge> <span className="hint">The SQL explorer reads the ClickHouse <code>events</code> table; configure <code>clickhouse.enabled</code> to use it.</span></div>}
      <div className="card">
        <Field label="Read-only SQL" hint="single SELECT / WITH statement on `events`; readonly=2, 30 s, row cap">
          <textarea className="mono" value={sql} onChange={(e) => setSql(e.target.value)} rows={8} style={{ width: '100%' }} spellCheck={false} />
        </Field>
        <div className="row" style={{ alignItems: 'end' }}>
          <Num label="Row limit" value={limit} onChange={(v) => setLimit(v ?? 1000)} min={1} max={st.data?.max_rows ?? 10000} />
          <button className="btn" disabled={busy || !sql.trim()} onClick={run}>{busy ? 'Running…' : 'Run'}</button>
          <div className="row" style={{ marginLeft: 'auto', alignItems: 'end' }}>
            <Text label="Save as" value={saveKey} onChange={setSaveKey} mono />
            <button className="btn ghost sm" disabled={!saveKey} onClick={save}>Save query</button>
            <Text label="Audience key" value={cohort} onChange={setCohort} mono />
            <button className="btn ghost sm" disabled={!cohort} onClick={toCohort} title="The result must have an identity_id column">To audience</button>
          </div>
        </div>
        {msg && <div className="hint">{msg}</div>}
      </div>
      <ErrorBox error={err} />
      {res && <ResultTable r={res} />}
    </div>
  );
}

function Queries() {
  const q = useLoad(() => get<{ items: SavedQuery[] }>('/analytics/queries'), []);
  const [res, setRes] = useState<{ key: string; r: SqlResult } | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const [edit, setEdit] = useState<SavedQuery | null>(null);
  const run = (key: string) => post<SqlResult>(`/analytics/queries/${encodeURIComponent(key)}/run`, {}).then((r) => { setRes({ key, r }); setErr(null); }, setErr);
  const save = () => edit && put(`/analytics/queries/${encodeURIComponent(edit.key)}`, { name: edit.name, description: edit.description, sql: edit.sql }).then(() => { setEdit(null); q.reload(); }, setErr);
  return (
    <div className="stack">
      <ErrorBox error={err ?? q.error} />
      <Table<SavedQuery>
        rows={q.data?.items}
        rowKey={(s) => s.key}
        empty="No saved queries — save one from the SQL tab."
        cols={[
          { h: 'Key', c: (s) => <strong className="mono">{s.key}</strong> },
          { h: 'Name', c: (s) => s.name },
          { h: 'SQL', c: (s) => <span className="mono hint" title={s.sql}>{s.sql.replace(/\s+/g, ' ').slice(0, 90)}</span> },
          { h: 'Updated', c: (s) => fmtDate(s.updated_at) },
          { h: '', c: (s) => <span className="row"><button className="btn ghost sm" onClick={() => run(s.key)}>Run</button><button className="btn ghost sm" onClick={() => setEdit(s)}>Edit</button><button className="btn ghost sm" onClick={() => del(`/analytics/queries/${encodeURIComponent(s.key)}`).then(q.reload, setErr)}>Delete</button></span> },
        ]}
      />
      {edit && (
        <div className="card stack">
          <strong className="mono">{edit.key}</strong>
          <div className="row"><Text label="Name" value={edit.name} onChange={(name) => setEdit({ ...edit, name })} /><Text label="Description" value={edit.description} onChange={(description) => setEdit({ ...edit, description })} /></div>
          <Field label="SQL"><textarea className="mono" rows={6} style={{ width: '100%' }} value={edit.sql} onChange={(e) => setEdit({ ...edit, sql: e.target.value })} spellCheck={false} /></Field>
          <div className="row"><button className="btn" onClick={save}>Save</button><button className="btn ghost" onClick={() => setEdit(null)}>Cancel</button></div>
        </div>
      )}
      {res && <><div className="hint">Result of <code>{res.key}</code></div><ResultTable r={res.r} /></>}
    </div>
  );
}

function PathsView() {
  const [event, setEvent] = useState('$session_start');
  const [direction, setDirection] = useState<'after' | 'before'>('after');
  const [depth, setDepth] = useState(3);
  const [gap, setGap] = useState(1800);
  const [days, setDays] = useState(7);
  const [res, setRes] = useState<Paths | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const run = () => post<Paths>('/analytics/paths', { event, direction, depth, max_gap_secs: gap, from: new Date(Date.now() - days * 86400e3).toISOString() }).then((d) => { setRes(d); setErr(null); }, setErr);
  return (
    <div className="stack">
      <div className="card row" style={{ alignItems: 'end' }}>
        <Text label="Anchor event" value={event} onChange={setEvent} mono />
        <Field label="Direction"><select value={direction} onChange={(e) => setDirection(e.target.value as 'after' | 'before')}><option value="after">what happens after</option><option value="before">what led to it</option></select></Field>
        <Num label="Steps" value={depth} onChange={(v) => setDepth(v ?? 3)} min={1} max={10} />
        <Num label="Max gap (s)" value={gap} onChange={(v) => setGap(v ?? 1800)} min={1} />
        <Num label="Last N days" value={days} onChange={(v) => setDays(v ?? 7)} min={1} max={90} />
        <button className="btn" disabled={!event} onClick={run}>Analyse</button>
      </div>
      <ErrorBox error={err} />
      {res && (
        <>
          <div className="hint">{fmtNum(res.anchors)} occurrences of <code>{res.event ?? event}</code>{res.truncated ? ' · computed on a capped sample' : ''}</div>
          <div className="grid" style={{ gridTemplateColumns: `repeat(${Math.max(1, res.levels.length)}, minmax(0, 1fr))`, gap: 12 }}>
            {res.levels.map((l) => (
              <div key={l.depth} className="card">
                <strong>Step {l.depth}</strong>
                {l.items.map((it) => (
                  <div key={it.name} className="row" style={{ alignItems: 'center', margin: '4px 0' }}>
                    <span className="mono" style={{ width: 150, overflow: 'hidden', textOverflow: 'ellipsis' }} title={it.name}>{it.name}</span>
                    <div style={{ flex: 1, background: 'var(--border)', height: 10, borderRadius: 3 }}><div style={{ width: `${it.share * 100}%`, background: '#1a5fb4', height: '100%', borderRadius: 3 }} /></div>
                    <span className="hint" style={{ width: 90, textAlign: 'right' }}>{fmtNum(it.count)} · {(it.share * 100).toFixed(1)}%</span>
                  </div>
                ))}
              </div>
            ))}
          </div>
          <div className="card">
            <strong>Top sequences</strong>
            <Table<Paths['sequences'][number]> rows={res.sequences} rowKey={(s) => s.steps.join('>')} cols={[{ h: 'Path', c: (s) => <span className="mono">{s.steps.join(' → ')}</span> }, { h: 'Players', c: (s) => fmtNum(s.count), w: '100px' }, { h: 'Share', c: (s) => `${((s.count / Math.max(1, res.anchors)) * 100).toFixed(1)}%`, w: '80px' }]} />
          </div>
        </>
      )}
    </div>
  );
}

const blankAlert = (): Omit<Alert, 'last_value' | 'last_checked_at' | 'last_fired_at'> => ({ key: '', name: '', enabled: true, event: '', aggregation: 'count', field: '', condition: { kind: 'below', threshold: 0 }, window_secs: 3600, cooldown_secs: 3600 });

function Alerts() {
  const q = useLoad(() => get<{ items: Alert[] }>('/analytics/alerts'), []);
  const [form, setForm] = useState(blankAlert());
  const [editing, setEditing] = useState(false);
  const [err, setErr] = useState<Error | null>(null);
  const [fires, setFires] = useState<{ key: string; items: Fire[] } | null>(null);
  const [check, setCheck] = useState<Record<string, unknown> | null>(null);
  const save = () => {
    const body = { ...form, field: form.aggregation === 'sum' ? form.field : '' };
    const p = editing ? put(`/analytics/alerts/${encodeURIComponent(form.key)}`, body) : post('/analytics/alerts', body);
    p.then(() => { setForm(blankAlert()); setEditing(false); setErr(null); q.reload(); }, setErr);
  };
  const c = form.condition;
  return (
    <div className="stack">
      <ErrorBox error={err ?? q.error} />
      <Table<Alert>
        rows={q.data?.items}
        rowKey={(a) => a.key}
        empty="No metric alerts yet."
        cols={[
          { h: 'Key', c: (a) => <strong className="mono">{a.key}</strong> },
          { h: 'Rule', c: (a) => <span>{a.aggregation}({a.event}{a.field ? `.${a.field}` : ''}) over {a.window_secs}s {a.condition.kind === 'anomaly' ? `anomaly z≥${a.condition.zscore}` : `${a.condition.kind} ${a.condition.threshold}`}</span> },
          { h: 'Last value', c: (a) => a.last_value === null ? '—' : fmtNum(a.last_value) },
          { h: 'Last fired', c: (a) => a.last_fired_at ? fmtDate(a.last_fired_at) : '—' },
          { h: 'State', c: (a) => <Badge tone={a.enabled ? 'ok' : 'muted'}>{a.enabled ? 'enabled' : 'disabled'}</Badge> },
          { h: '', c: (a) => <span className="row"><button className="btn ghost sm" onClick={() => post<Record<string, unknown>>(`/analytics/alerts/${encodeURIComponent(a.key)}/check`, {}).then((d) => { setCheck(d); q.reload(); }, setErr)}>Check</button><button className="btn ghost sm" onClick={() => get<{ items: Fire[] }>(`/analytics/alerts/${encodeURIComponent(a.key)}/fires`).then((d) => setFires({ key: a.key, items: d.items }), setErr)}>History</button><button className="btn ghost sm" onClick={() => { setForm({ ...a }); setEditing(true); }}>Edit</button><button className="btn ghost sm" onClick={() => del(`/analytics/alerts/${encodeURIComponent(a.key)}`).then(q.reload, setErr)}>Delete</button></span> },
        ]}
      />
      {check && <div className="card hint">Check: value {fmtNum(check.value as number)} · {check.fired ? 'fired' : check.suppressed ? 'condition met, suppressed by cooldown' : 'ok'}</div>}
      {fires && <div className="card"><strong>Fires of <code>{fires.key}</code></strong><Table<Fire> rows={fires.items} rowKey={(f) => f.id} empty="Never fired." cols={[{ h: 'When', c: (f) => fmtDate(f.fired_at) }, { h: 'Value', c: (f) => fmtNum(f.value) }, { h: 'Baseline', c: (f) => f.baseline === null ? '—' : fmtNum(f.baseline) }, { h: 'Text', c: (f) => f.text }]} /></div>}
      <div className="card stack">
        <strong>{editing ? `Edit ${form.key}` : 'New alert'}</strong>
        <div className="row" style={{ alignItems: 'end' }}>
          {!editing && <Text label="Key" value={form.key} onChange={(key) => setForm({ ...form, key })} mono />}
          <Text label="Name" value={form.name} onChange={(name) => setForm({ ...form, name })} />
          <Text label="Event" value={form.event} onChange={(event) => setForm({ ...form, event })} mono />
          <Field label="Aggregation"><select value={form.aggregation} onChange={(e) => setForm({ ...form, aggregation: e.target.value as Alert['aggregation'] })}><option value="count">count</option><option value="unique">unique players</option><option value="sum">sum of field</option></select></Field>
          {form.aggregation === 'sum' && <Text label="Metadata field" value={form.field} onChange={(field) => setForm({ ...form, field })} mono />}
          <Num label="Window (s)" value={form.window_secs} onChange={(v) => setForm({ ...form, window_secs: v ?? 3600 })} min={60} />
          <Num label="Cooldown (s)" value={form.cooldown_secs} onChange={(v) => setForm({ ...form, cooldown_secs: v ?? 3600 })} min={0} />
          <Toggle label="Enabled" value={form.enabled} onChange={(enabled) => setForm({ ...form, enabled })} />
        </div>
        <div className="row" style={{ alignItems: 'end' }}>
          <Field label="Condition"><select value={c.kind} onChange={(e) => setForm({ ...form, condition: e.target.value === 'anomaly' ? { kind: 'anomaly', zscore: 3, baseline_windows: 7, min_delta: 0 } : { kind: e.target.value as 'above' | 'below', threshold: 0 } })}><option value="below">below threshold</option><option value="above">above threshold</option><option value="anomaly">anomaly vs previous windows</option></select></Field>
          {c.kind !== 'anomaly' && <Num label="Threshold" value={c.threshold} onChange={(v) => setForm({ ...form, condition: { kind: c.kind, threshold: v ?? 0 } })} />}
          {c.kind === 'anomaly' && <><Num label="z-score" value={c.zscore} onChange={(v) => setForm({ ...form, condition: { ...c, zscore: v ?? 3 } })} min={0.5} /><Num label="Baseline windows" value={c.baseline_windows} onChange={(v) => setForm({ ...form, condition: { ...c, baseline_windows: v ?? 7 } })} min={3} max={60} /><Num label="Min delta" value={c.min_delta} onChange={(v) => setForm({ ...form, condition: { ...c, min_delta: v ?? 0 } })} min={0} /></>}
          <button className="btn" disabled={!form.key || !form.event} onClick={save}>{editing ? 'Save' : 'Create'}</button>
          {editing && <button className="btn ghost" onClick={() => { setForm(blankAlert()); setEditing(false); }}>Cancel</button>}
        </div>
        <div className="hint">Alerts are evaluated every minute by housekeeping and delivered to notification channels subscribed to <code>alert</code>.</div>
      </div>
    </div>
  );
}

const blankReport = (): Omit<Report, 'last_run_at' | 'last_status'> => ({ key: '', name: '', enabled: true, query_key: '', cron: '0 0 9 * * *', timezone: 'UTC', channels: [], email: null, max_rows: 20 });

function Reports() {
  const q = useLoad(() => get<{ items: Report[] }>('/analytics/reports'), []);
  const queries = useLoad(() => get<{ items: SavedQuery[] }>('/analytics/queries'), []);
  const [form, setForm] = useState(blankReport());
  const [editing, setEditing] = useState(false);
  const [err, setErr] = useState<Error | null>(null);
  const [out, setOut] = useState<Record<string, unknown> | null>(null);
  const [emailOn, setEmailOn] = useState(false);
  const save = () => {
    const body = { ...form, email: emailOn ? form.email : null };
    const p = editing ? put(`/analytics/reports/${encodeURIComponent(form.key)}`, body) : post('/analytics/reports', body);
    p.then(() => { setForm(blankReport()); setEditing(false); setEmailOn(false); setErr(null); q.reload(); }, setErr);
  };
  const email = form.email ?? { integration: '', to: [] };
  return (
    <div className="stack">
      <ErrorBox error={err ?? q.error} />
      <Table<Report>
        rows={q.data?.items}
        rowKey={(r) => r.key}
        empty="No scheduled reports yet."
        cols={[
          { h: 'Key', c: (r) => <strong className="mono">{r.key}</strong> },
          { h: 'Query', c: (r) => <span className="mono">{r.query_key}</span> },
          { h: 'Schedule', c: (r) => <span className="mono">{r.cron} <span className="hint">{r.timezone}</span></span> },
          { h: 'Delivery', c: (r) => <span className="hint">{r.channels.length ? r.channels.join(', ') : 'all report channels'}{r.email ? ` · email via ${r.email.integration} (${r.email.to.length})` : ''}</span> },
          { h: 'Last run', c: (r) => r.last_run_at ? <span>{fmtDate(r.last_run_at)} <Badge tone={r.last_status.startsWith('error') ? 'bad' : r.last_status === 'partial' ? 'warn' : 'ok'}>{r.last_status || 'ok'}</Badge></span> : '—' },
          { h: 'State', c: (r) => <Badge tone={r.enabled ? 'ok' : 'muted'}>{r.enabled ? 'enabled' : 'disabled'}</Badge> },
          { h: '', c: (r) => <span className="row"><button className="btn ghost sm" onClick={() => post<Record<string, unknown>>(`/analytics/reports/${encodeURIComponent(r.key)}/run`, {}).then((d) => { setOut(d); setErr(null); q.reload(); }, setErr)}>Run now</button><button className="btn ghost sm" onClick={() => { setForm({ ...r }); setEmailOn(!!r.email); setEditing(true); }}>Edit</button><button className="btn ghost sm" onClick={() => del(`/analytics/reports/${encodeURIComponent(r.key)}`).then(q.reload, setErr)}>Delete</button></span> },
        ]}
      />
      {out && <div className="card hint">Run: {String(out.status)} · {String(out.rows)} rows</div>}
      <div className="card stack">
        <strong>{editing ? `Edit ${form.key}` : 'New report'}</strong>
        <div className="row" style={{ alignItems: 'end' }}>
          {!editing && <Text label="Key" value={form.key} onChange={(key) => setForm({ ...form, key })} mono />}
          <Text label="Name" value={form.name} onChange={(name) => setForm({ ...form, name })} />
          <Field label="Saved query"><select value={form.query_key} onChange={(e) => setForm({ ...form, query_key: e.target.value })}><option value="">—</option>{(queries.data?.items ?? []).map((s) => <option key={s.key} value={s.key}>{s.key}</option>)}</select></Field>
          <Text label="Cron (sec min hour dom mon dow)" value={form.cron} onChange={(cron) => setForm({ ...form, cron })} mono />
          <Text label="Timezone" value={form.timezone} onChange={(timezone) => setForm({ ...form, timezone })} mono />
          <Num label="Rows" value={form.max_rows} onChange={(v) => setForm({ ...form, max_rows: v ?? 20 })} min={1} max={200} />
          <Toggle label="Enabled" value={form.enabled} onChange={(enabled) => setForm({ ...form, enabled })} />
        </div>
        <div className="row" style={{ alignItems: 'end' }}>
          <Text label="Notification channels (comma-separated keys, empty = all subscribed to `report`)" value={form.channels.join(', ')} onChange={(v) => setForm({ ...form, channels: v.split(',').map((x) => x.trim()).filter(Boolean) })} mono />
          <Toggle label="Email" value={emailOn} onChange={setEmailOn} />
          {emailOn && <><Text label="Email integration key" value={email.integration} onChange={(integration) => setForm({ ...form, email: { ...email, integration } })} mono /><Text label="Recipients (comma-separated)" value={email.to.join(', ')} onChange={(v) => setForm({ ...form, email: { ...email, to: v.split(',').map((x) => x.trim()).filter(Boolean) } })} mono /></>}
          <button className="btn" disabled={!form.key || !form.query_key} onClick={save}>{editing ? 'Save' : 'Create'}</button>
          {editing && <button className="btn ghost" onClick={() => { setForm(blankReport()); setEditing(false); }}>Cancel</button>}
        </div>
      </div>
    </div>
  );
}
