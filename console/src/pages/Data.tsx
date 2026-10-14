import { useState } from 'react';
import { del, get, post, useLoad } from '../api';
import { EntityPage, Num, Tags, Text } from '../crud';
import { ErrorBox, Field, JsonField, Page, Table, fmtDate, fmtNum } from '../ui';

type Sink = { id: string; key: string; name: string; kind: 'http' | 's3' | 'bigquery' | 'snowflake' | 'redshift'; config: Record<string, unknown>; format: 'jsonl' | 'csv' | 'parquet'; gzip: boolean; enabled: boolean; events: string[]; interval_secs: number; batch_size: number; cursor_at: string | null; last_run_at: string | null; last_error: string; tags: string[]; version: number; updated_at: string };
type Run = { id: string; started_at: string; finished_at: string | null; status: string; rows: number; bytes: number; object: string; error: string };
type Snapshot = { id: string; kind: string; key: string; version: number; body: unknown; deleted: boolean; actor: string; at: string };

const KIND_FIELDS: Record<Sink['kind'], [string, string][]> = {
  http: [['url', 'URL'], ['secret', 'HMAC secret (optional)']],
  s3: [['bucket', 'Bucket'], ['region', 'Region'], ['access_key', 'Access key'], ['secret_key', 'Secret key'], ['endpoint', 'Endpoint (S3-compatible, optional)'], ['prefix', 'Key prefix']],
  bigquery: [['project', 'Project'], ['dataset', 'Dataset'], ['table', 'Table']],
  snowflake: [['bucket', 'Stage bucket'], ['region', 'Bucket region'], ['access_key', 'AWS access key'], ['secret_key', 'AWS secret key'], ['prefix', 'Key prefix (= stage URL path)'], ['account', 'Account identifier'], ['host', 'Host (xy12345.eu-central-1.snowflakecomputing.com)'], ['user', 'User'], ['public_key_fp', 'Public key fingerprint (SHA256:…)'], ['database', 'Database'], ['schema', 'Schema'], ['table', 'Table'], ['stage', 'External stage'], ['warehouse', 'Warehouse (optional)'], ['role', 'Role (optional)']],
  redshift: [['bucket', 'Staging bucket'], ['region', 'Region'], ['access_key', 'AWS access key'], ['secret_key', 'AWS secret key'], ['prefix', 'Key prefix'], ['database', 'Database'], ['table', 'Table'], ['iam_role', 'IAM role ARN (COPY)'], ['cluster_id', 'Cluster id (provisioned)'], ['workgroup', 'Workgroup (serverless)'], ['db_user', 'DB user (optional)'], ['secret_arn', 'Secrets Manager ARN (optional)']],
};

function SinkRuns({ s }: { s: Sink }) {
  const q = useLoad(() => get<{ items: Run[] }>(`/export-sinks/${encodeURIComponent(s.key)}/runs`), [s.key, s.version]);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const act = (p: string) => post<Record<string, unknown>>(`/export-sinks/${encodeURIComponent(s.key)}/${p}`, {}).then((r) => { setMsg(JSON.stringify(r)); setErr(null); q.reload(); }, (e) => { setErr(e); setMsg(null); });
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}>
        <strong>Runs</strong>
        <span className="hint">cursor {s.cursor_at ? fmtDate(s.cursor_at) : 'start'} · last run {s.last_run_at ? fmtDate(s.last_run_at) : '—'}</span>
        <button className="btn sm" onClick={() => act('run')}>Run now</button>
        <button className="btn ghost sm" onClick={() => act('test')}>Send test</button>
        <button className="btn ghost sm" onClick={() => act('reset-cursor')}>Reset cursor</button>
      </div>
      {s.last_error && <ErrorBox error={new Error(s.last_error)} />}
      <ErrorBox error={err ?? q.error} />
      {msg && <pre className="mono hint" style={{ whiteSpace: 'pre-wrap' }}>{msg}</pre>}
      <Table<Run> rows={q.data?.items} rowKey={(r) => r.id} empty="No runs yet" cols={[
        { h: 'Started', c: (r) => fmtDate(r.started_at) }, { h: 'Status', c: (r) => <code>{r.status}</code> }, { h: 'Rows', c: (r) => fmtNum(r.rows) },
        { h: 'Bytes', c: (r) => fmtNum(r.bytes) }, { h: 'Object', c: (r) => <span className="mono">{r.object}</span> }, { h: 'Error', c: (r) => r.error },
      ]} />
    </div>
  );
}

function Sinks() {
  return (
    <EntityPage<Sink>
      title="Export sinks"
      path="/export-sinks"
      blurb="Ship raw events to S3-compatible storage (JSONL / CSV / Parquet, optionally one object per event), an HTTP endpoint, BigQuery streaming, Snowflake (stage + COPY INTO) or Redshift (S3 + COPY). Rejected events carry `_error`. Each sink keeps its own cursor and run log."
      cols={[{ h: 'Key', c: (s) => <><strong>{s.key}</strong><div className="hint">{s.name}</div></> }, { h: 'Kind', c: (s) => <code>{s.kind}</code> }, { h: 'Every', c: (s) => `${s.interval_secs}s` }, { h: 'Last run', c: (s) => s.last_run_at ? fmtDate(s.last_run_at) : '—' }, { h: 'State', c: (s) => s.last_error ? <span style={{ color: 'var(--danger)' }}>error</span> : s.enabled ? 'on' : 'off' }]}
      blank={() => ({ key: '', name: '', kind: 'http', config: {}, format: 'jsonl', gzip: true, enabled: true, events: [], interval_secs: 300, batch_size: 10000, tags: [] })}
      form={(s, set, isNew) => (
        <>
          <div className="row"><Text label="Key" value={s.key} onChange={(key) => set({ key })} disabled={!isNew} mono /><Text label="Name" value={s.name} onChange={(name) => set({ name })} /></div>
          <div className="row">
            <Field label="Kind"><select value={s.kind ?? 'http'} onChange={(e) => set({ kind: e.target.value as Sink['kind'], config: {} })}><option value="http">HTTP endpoint</option><option value="s3">S3 / compatible</option><option value="snowflake">Snowflake</option><option value="redshift">Redshift</option><option value="bigquery">BigQuery</option></select></Field>
            <Field label="Format"><select value={s.format ?? 'jsonl'} onChange={(e) => set({ format: e.target.value as Sink['format'] })} disabled={s.kind === 'bigquery'}><option value="jsonl">JSON lines</option><option value="csv">CSV</option><option value="parquet">Parquet (snappy)</option></select></Field>
            <Field label="gzip"><input type="checkbox" checked={s.gzip ?? true} onChange={(e) => set({ gzip: e.target.checked })} disabled={s.format === 'parquet'} /></Field>
            {s.kind !== 'http' && s.kind !== 'bigquery' && <Field label="Object per event"><input type="checkbox" checked={(s.config ?? {}).partition === 'event'} onChange={(e) => set({ config: { ...(s.config ?? {}), partition: e.target.checked ? 'event' : 'none' } })} /></Field>}
            <Field label="Enabled"><input type="checkbox" checked={s.enabled ?? true} onChange={(e) => set({ enabled: e.target.checked })} /></Field>
          </div>
          <div className="row">
            {KIND_FIELDS[s.kind ?? 'http'].map(([k, l]) => <Text key={k} label={l} value={((s.config ?? {})[k] as string) ?? ''} onChange={(v) => set({ config: { ...(s.config ?? {}), [k]: v } })} mono />)}
          </div>
          {s.kind === 'snowflake' && <Field label="Private key (PEM, key-pair auth)"><textarea rows={4} value={((s.config ?? {}).private_key as string) ?? ''} onChange={(e) => set({ config: { ...(s.config ?? {}), private_key: e.target.value } })} /></Field>}
          {s.kind === 'bigquery' && <Field label="Service account JSON"><JsonField value={((s.config ?? {}).service_account as object) ?? {}} onChange={(service_account) => set({ config: { ...(s.config ?? {}), service_account } })} rows={4} /></Field>}
          {s.kind === 'http' && <Field label="Extra headers (JSON object)"><JsonField value={((s.config ?? {}).headers as object) ?? {}} onChange={(headers) => set({ config: { ...(s.config ?? {}), headers } })} rows={2} /></Field>}
          <div className="row">
            <Text label="Events (comma-separated, empty = all)" value={(s.events ?? []).join(',')} onChange={(v) => set({ events: v.split(',').map((x) => x.trim()).filter(Boolean) })} mono />
            <Num label="Interval (s)" value={s.interval_secs ?? 300} onChange={(v) => set({ interval_secs: v ?? 300 })} min={30} />
            <Num label="Batch size" value={s.batch_size ?? 10000} onChange={(v) => set({ batch_size: v ?? 10000 })} min={1} max={100000} />
          </div>
          <Tags value={s.tags ?? []} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(s) => <SinkRuns s={s} />}
    />
  );
}

type Diff = { kinds: Record<string, { added: { key: string }[]; changed: { key: string; fields: string[] }[]; removed: { key: string }[]; unchanged: number }>; summary: Record<string, number> };

function ConfigTab() {
  const kinds = useLoad(() => get<{ kinds: string[] }>('/config/kinds'), []);
  const [bundle, setBundle] = useState<unknown>(null);
  const [mode, setMode] = useState<'merge' | 'replace'>('merge');
  const [diff, setDiff] = useState<Diff | null>(null);
  const [result, setResult] = useState<unknown>(null);
  const [err, setErr] = useState<Error | null>(null);
  const [remote, setRemote] = useState({ url: '', admin_key: '' });
  const download = () => get<unknown>('/config/export').then((b) => {
    const blob = new Blob([JSON.stringify(b, null, 2)], { type: 'application/json' });
    const a = document.createElement('a'); a.href = URL.createObjectURL(blob); a.download = `zanshin-config-${new Date().toISOString().slice(0, 10)}.json`; a.click();
  }, setErr);
  const onFile = (f: File | undefined) => f?.text().then((t) => { setBundle(JSON.parse(t)); setDiff(null); setResult(null); }).catch(setErr);
  const runDiff = () => post<Diff>('/config/diff', { bundle, mode }).then((d) => { setDiff(d); setErr(null); }, setErr);
  const runImport = (dry_run: boolean) => post<unknown>('/config/import', { bundle, mode, dry_run }).then((r) => { setResult(r); setErr(null); }, setErr);
  const importFrom = (dry_run: boolean) => post<unknown>('/config/import-from', { ...remote, mode, dry_run }).then((r) => { setResult(r); setErr(null); }, setErr);
  return (
    <div className="stack">
      <div className="card">
        <strong>Export</strong>
        <p className="hint">Download the whole configuration ({kinds.data?.kinds.join(', ')}) as one JSON bundle. Volatile fields (ids, versions, timestamps) are stripped so bundles can move between servers.</p>
        <button className="btn" onClick={download}>Download bundle</button>
      </div>
      <div className="card stack">
        <strong>Import</strong>
        <div className="row" style={{ alignItems: 'end' }}>
          <Field label="Bundle file"><input type="file" accept="application/json" onChange={(e) => onFile(e.target.files?.[0])} /></Field>
          <Field label="Mode"><select value={mode} onChange={(e) => setMode(e.target.value as 'merge' | 'replace')}><option value="merge">Merge (create + update)</option><option value="replace">Replace (also delete entities missing from the bundle)</option></select></Field>
          <button className="btn ghost" disabled={!bundle} onClick={runDiff}>Preview diff</button>
          <button className="btn" disabled={!bundle} onClick={() => runImport(false)}>Import</button>
        </div>
        <div className="row" style={{ alignItems: 'end' }}>
          <Text label="Or import from another server (base URL)" value={remote.url} onChange={(url) => setRemote({ ...remote, url })} mono placeholder="https://staging.example.com" />
          <Text label="Its admin key" value={remote.admin_key} onChange={(admin_key) => setRemote({ ...remote, admin_key })} mono />
          <button className="btn ghost" disabled={!remote.url || !remote.admin_key} onClick={() => importFrom(true)}>Dry run</button>
          <button className="btn" disabled={!remote.url || !remote.admin_key} onClick={() => importFrom(false)}>Import</button>
        </div>
        <ErrorBox error={err} />
        {diff && (
          <div>
            <div className="hint">+{diff.summary.added} added · ~{diff.summary.changed} changed · −{diff.summary.removed} removed · {diff.summary.unchanged} unchanged</div>
            {Object.entries(diff.kinds).filter(([, k]) => k.added.length + k.changed.length + k.removed.length > 0).map(([kind, k]) => (
              <div key={kind} style={{ marginTop: 6 }}><strong>{kind}</strong>
                <ul className="hint" style={{ margin: '4px 0 0 16px' }}>
                  {k.added.map((a) => <li key={'a' + a.key} style={{ color: 'var(--ok, #1e8e3e)' }}>+ {a.key}</li>)}
                  {k.changed.map((a) => <li key={'c' + a.key}>~ {a.key} <span className="mono">({a.fields.join(', ')})</span></li>)}
                  {k.removed.map((a) => <li key={'r' + a.key} style={{ color: 'var(--danger)' }}>− {a.key}</li>)}
                </ul>
              </div>
            ))}
          </div>
        )}
        {result != null && <pre className="mono hint" style={{ whiteSpace: 'pre-wrap', maxHeight: 240, overflow: 'auto' }}>{JSON.stringify(result, null, 2)}</pre>}
      </div>
    </div>
  );
}

function HistoryTab() {
  const [kind, setKind] = useState('');
  const [key, setKey] = useState('');
  const q = useLoad(() => get<{ items: Snapshot[] }>(`/config/history?limit=200${kind ? `&kind=${encodeURIComponent(kind)}` : ''}${key ? `&key=${encodeURIComponent(key)}` : ''}`), [kind, key]);
  const [err, setErr] = useState<Error | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const revert = (id: string) => post(`/config/history/${id}/revert`, {}).then(() => { setErr(null); q.reload(); }, setErr);
  return (
    <div className="stack">
      <p className="hint">Every configuration change is snapshotted. Revert restores an entity to how it looked in that snapshot (or deletes it again if the snapshot is a deletion).</p>
      <div className="row" style={{ alignItems: 'end' }}><Text label="Kind" value={kind} onChange={setKind} mono placeholder="flags" /><Text label="Key" value={key} onChange={setKey} mono /></div>
      <ErrorBox error={err ?? q.error} />
      <Table<Snapshot> rows={q.data?.items} rowKey={(s) => s.id} empty="No history yet" cols={[
        { h: 'When', c: (s) => fmtDate(s.at) }, { h: 'Kind', c: (s) => <code>{s.kind}</code> }, { h: 'Key', c: (s) => <strong>{s.key}</strong> },
        { h: 'Change', c: (s) => s.deleted ? <span style={{ color: 'var(--danger)' }}>deleted</span> : `v${s.version}${s.actor ? ` · ${s.actor}` : ''}` },
        { h: '', c: (s) => <span className="row"><button className="btn ghost sm" onClick={() => setOpen(open === s.id ? null : s.id)}>{open === s.id ? 'Hide' : 'Show'}</button><button className="btn sm" onClick={() => revert(s.id)}>Revert to this</button></span> },
      ]} />
      {open && <pre className="mono hint" style={{ whiteSpace: 'pre-wrap', maxHeight: 320, overflow: 'auto' }}>{JSON.stringify(q.data?.items.find((s) => s.id === open)?.body, null, 2)}</pre>}
    </div>
  );
}

function ToolsTab() {
  const [sample, setSample] = useState({ players: 200, days: 14, seed: 1 });
  const [purge, setPurge] = useState({ before: new Date(Date.now() - 400 * 86400e3).toISOString().slice(0, 10), names: '' });
  const [pid, setPid] = useState('');
  const [out, setOut] = useState<unknown>(null);
  const [err, setErr] = useState<Error | null>(null);
  const show = (p: Promise<unknown>) => p.then((r) => { setOut(r); setErr(null); }, (e) => { setErr(e); setOut(null); });
  return (
    <div className="stack">
      <div className="card">
        <strong>Sample data</strong>
        <p className="hint">Generate a deterministic population (sessions, level-ups, purchases, retention decay) to try out analytics on an empty server. Players get external ids <code>sample-&lt;seed&gt;-N</code> and <code>custom.sample = true</code>.</p>
        <div className="row" style={{ alignItems: 'end' }}>
          <Num label="Players" value={sample.players} onChange={(v) => setSample({ ...sample, players: v ?? 1 })} min={1} max={5000} />
          <Num label="Days" value={sample.days} onChange={(v) => setSample({ ...sample, days: v ?? 1 })} min={1} max={90} />
          <Num label="Seed" value={sample.seed} onChange={(v) => setSample({ ...sample, seed: v ?? 0 })} min={0} />
          <button className="btn" onClick={() => show(post('/data/sample', sample))}>Generate</button>
        </div>
      </div>
      <div className="card">
        <strong>Player data (GDPR)</strong>
        <div className="row" style={{ alignItems: 'end' }}>
          <Text label="Identity id" value={pid} onChange={setPid} mono />
          <button className="btn ghost" disabled={!pid} onClick={() => show(get(`/data/identities/${pid}/takeout`))}>Takeout (JSON)</button>
          <button className="btn danger" disabled={!pid} onClick={() => confirm('Delete this player and all their events?') && show(del(`/data/identities/${pid}`))}>Delete everything</button>
        </div>
      </div>
      <div className="card">
        <strong>Purge events</strong>
        <div className="row" style={{ alignItems: 'end' }}>
          <Text label="Older than (date)" value={purge.before} onChange={(before) => setPurge({ ...purge, before })} mono />
          <Text label="Only these events (comma-separated, optional)" value={purge.names} onChange={(names) => setPurge({ ...purge, names })} mono />
          <button className="btn danger" onClick={() => confirm('Permanently delete matching events?') && show(post('/data/events/purge', { before: new Date(purge.before).toISOString(), names: purge.names.split(',').map((x) => x.trim()).filter(Boolean) }))}>Purge</button>
        </div>
      </div>
      <ErrorBox error={err} />
      {out != null && <pre className="mono hint" style={{ whiteSpace: 'pre-wrap', maxHeight: 320, overflow: 'auto' }}>{JSON.stringify(out, null, 2)}</pre>}
    </div>
  );
}

export default function Data() {
  const [tab, setTab] = useState<'sinks' | 'config' | 'history' | 'tools'>('sinks');
  const tabs: [typeof tab, string][] = [['sinks', 'Export sinks'], ['config', 'Config export / import'], ['history', 'History'], ['tools', 'Data tools']];
  const chips = <div className="chips" style={{ marginBottom: 12 }}>{tabs.map(([k, l]) => <button key={k} className={'chip' + (k === tab ? ' on' : '')} onClick={() => setTab(k)}>{l}</button>)}</div>;
  if (tab === 'sinks') return <>{chips}<Sinks /></>;
  return (
    <Page title="Data">
      {chips}
      {tab === 'config' && <ConfigTab />}
      {tab === 'history' && <HistoryTab />}
      {tab === 'tools' && <ToolsTab />}
    </Page>
  );
}
