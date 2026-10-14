import { useState } from 'react';
import { NavLink, Navigate, Route, Routes } from 'react-router-dom';
import { del, get, post, put, qs, useLoad } from '../api';
import { Badge, Confirm, ErrorBox, Field, JsonField, Page, Table, Toggle, fmtDate, fmtNum } from '../ui';

type Label = { id: string; name: string; color: string; description: string; created_at: string };
type EventDef = {
  name: string; description: string; value_type: string; metadata_schema: unknown; exclude_from_activity: boolean;
  is_core: boolean; discovered: boolean; tags: string[]; updated_at: string;
};
type Rejected = { at: string; external_id: string | null; name: string; value: string | null; metadata: unknown; reason: string; rejected: boolean };
type PropCfg = { key: string; namespace: string; read_only: boolean; description: string };
type Observed = { key: string; json_type: string; identities: number };
type Schema = { id: string; name: string; description: string; schema: unknown; updated_at: string };
type Settings = { mode: 'off' | 'warn' | 'strict'; auto_discover: boolean; geoip_enabled: boolean; session_inactivity_secs: number };

const TABS: [string, string][] = [['events', 'Events'], ['properties', 'Properties'], ['schemas', 'Value schemas'], ['labels', 'Labels']];

export default function Taxonomy() {
  return (
    <Page title="Taxonomy">
      <p className="muted" style={{ marginTop: -8 }}>
        Define what your game sends: event names and validators, player properties, JSON schemas for flag values and the labels used to organise everything.
      </p>
      <nav className="tabs">{TABS.map(([p, l]) => <NavLink key={p} to={p}>{l}</NavLink>)}</nav>
      <Routes>
        <Route path="events" element={<Events />} />
        <Route path="properties" element={<Properties />} />
        <Route path="schemas" element={<Schemas />} />
        <Route path="labels" element={<Labels />} />
        <Route path="*" element={<Navigate to="events" replace />} />
      </Routes>
    </Page>
  );
}

function useErr() {
  const [error, setError] = useState<Error | null>(null);
  const run = async (f: () => Promise<unknown>) => { setError(null); try { await f(); } catch (e) { setError(e as Error); } };
  return { error, run };
}

// ---- events -----------------------------------------------------------------------------------

const EMPTY_DEF: Partial<EventDef> = { name: '', description: '', value_type: 'any', metadata_schema: null, exclude_from_activity: false, tags: [] };

type Rate = { day: string; code: string; usd_per_unit: number; source: string };

function CurrencyRates() {
  const q = useLoad(() => get<{ items: Rate[]; currencies: string[]; remote_configured: boolean }>('/currency-rates'));
  const [open, setOpen] = useState(false);
  const [code, setCode] = useState('EUR');
  const [rate, setRate] = useState('1.08');
  const { error, run } = useErr();
  const latest = new Map<string, Rate>();
  for (const r of q.data?.items ?? []) latest.set(r.code, r);
  const add = () => run(async () => { await put('/currency-rates', { rates: { [code.toUpperCase()]: Number(rate) } }); q.reload(); });
  const refresh = () => run(async () => { await post('/currency-rates/refresh', {}); q.reload(); });
  return (
    <div className="card stack">
      <div className="row" style={{ alignItems: 'center' }}>
        <strong>Currency → USD rates</strong>
        <span className="hint">{latest.size} currencies · revenue metadata gets <code>amount_usd</code> at ingest · feed {q.data?.remote_configured ? 'configured' : 'not configured (taxonomy.currency_rates_url)'}</span>
        <div className="spacer" />
        {q.data?.remote_configured && <button className="btn secondary" onClick={refresh}>Refresh from feed</button>}
        <button className="btn secondary" onClick={() => setOpen(!open)}>{open ? 'Hide' : 'Show'}</button>
      </div>
      <ErrorBox error={error ?? q.error} />
      {open && (
        <>
          <div className="row" style={{ alignItems: 'end' }}>
            <Field label="Code"><input value={code} onChange={(e) => setCode(e.target.value)} style={{ width: 80 }} /></Field>
            <Field label="USD per unit"><input value={rate} onChange={(e) => setRate(e.target.value)} style={{ width: 120 }} /></Field>
            <button className="btn" onClick={add} disabled={!/^[A-Za-z]{3}$/.test(code) || !(Number(rate) > 0)}>Set today's rate</button>
          </div>
          <Table<Rate> rows={Array.from(latest.values())} rowKey={(r) => r.code} empty="No rates yet — USD amounts are taken as-is" cols={[
            { h: 'Code', c: (r) => <code>{r.code}</code> }, { h: 'USD per unit', c: (r) => r.usd_per_unit }, { h: 'Day', c: (r) => r.day }, { h: 'Source', c: (r) => r.source },
          ]} />
        </>
      )}
    </div>
  );
}

function Events() {
  const [showCore, setShowCore] = useState(true);
  const [showSynthetic, setShowSynthetic] = useState(false);
  const [onlyDiscovered, setOnlyDiscovered] = useState(false);
  const defs = useLoad(() => get<EventDef[]>('/taxonomy/events' + qs({ core: showCore ? undefined : false, synthetic: showSynthetic ? undefined : false, discovered: onlyDiscovered ? true : undefined })), [showCore, showSynthetic, onlyDiscovered]);
  const settings = useLoad(() => get<Settings>('/taxonomy/settings'));
  const rejected = useLoad(() => get<Rejected[]>('/taxonomy/rejected' + qs({ limit: 100 })));
  const [edit, setEdit] = useState<Partial<EventDef> | null>(null);
  const [filter, setFilter] = useState('');
  const { error, run } = useErr();
  const save = () => run(async () => {
    const { name, ...rest } = edit!;
    await put(`/taxonomy/events/${encodeURIComponent(name!)}`, rest);
    setEdit(null); defs.reload();
  });
  const setMode = (mode: Settings['mode']) => run(async () => { await put('/taxonomy/settings', { mode }); settings.reload(); });
  const rows = (defs.data ?? []).filter((d) => !filter || d.name.toLowerCase().includes(filter.toLowerCase()));
  return (
    <>
      <div className="card row" style={{ alignItems: 'end' }}>
        <Field label="Enforcement" hint="warn: store but report violations · strict: reject invalid events · off: accept anything">
          <select value={settings.data?.mode ?? 'warn'} onChange={(e) => setMode(e.target.value as Settings['mode'])}>
            <option value="off">off</option><option value="warn">warn</option><option value="strict">strict</option>
          </select>
        </Field>
        <div className="field"><span className="label">Auto-discover</span><Badge tone={settings.data?.auto_discover ? 'ok' : 'muted'}>{settings.data?.auto_discover ? 'on' : 'off'}</Badge></div>
        <div className="field"><span className="label">Geo-IP</span><Badge tone={settings.data?.geoip_enabled ? 'ok' : 'muted'}>{settings.data?.geoip_enabled ? 'enabled' : 'not configured'}</Badge></div>
        <div className="field"><span className="label">Session inactivity</span><span>{settings.data ? `${Math.round(settings.data.session_inactivity_secs / 60)} min` : '…'}</span></div>
        <div className="spacer" />
        <label className="hint" style={{ whiteSpace: 'nowrap' }}><input type="checkbox" checked={showCore} onChange={(e) => setShowCore(e.target.checked)} /> Show core</label>
        <label className="hint" style={{ whiteSpace: 'nowrap' }}><input type="checkbox" checked={showSynthetic} onChange={(e) => setShowSynthetic(e.target.checked)} /> Show synthetic</label>
        <label className="hint" style={{ whiteSpace: 'nowrap' }}><input type="checkbox" checked={onlyDiscovered} onChange={(e) => setOnlyDiscovered(e.target.checked)} /> Only discovered</label>
        <input placeholder="Filter events" value={filter} onChange={(e) => setFilter(e.target.value)} style={{ maxWidth: 220 }} />
        <button className="btn" onClick={() => setEdit({ ...EMPTY_DEF })}>New event</button>
      </div>
      <CurrencyRates />
      <ErrorBox error={error ?? defs.error ?? settings.error} />
      {edit && (
        <div className="card">
          <h3>{edit.is_core ? `Core event ${edit.name}` : edit.name ? `Edit ${edit.name}` : 'New event definition'}</h3>
          <div className="row">
            <label className="field"><span className="label">Name</span><input className="mono" value={edit.name ?? ''} disabled={!!edit.updated_at} onChange={(e) => setEdit({ ...edit, name: e.target.value })} placeholder="levelCompleted" /></label>
            <Field label="Value type" hint="number · string · object (JSON) · revenue (metadata.amount + metadata.currency) · any">
              <select value={edit.value_type} onChange={(e) => setEdit({ ...edit, value_type: e.target.value })}>
                {['any', 'number', 'string', 'object', 'revenue'].map((t) => <option key={t}>{t}</option>)}
              </select>
            </Field>
            <label className="field grow"><span className="label">Description</span><input value={edit.description ?? ''} onChange={(e) => setEdit({ ...edit, description: e.target.value })} /></label>
          </div>
          <Field label="Metadata JSON Schema" hint="Optional. Events whose metadata does not match are reported (warn) or rejected (strict).">
            <JsonField value={edit.metadata_schema ?? null} onChange={(v) => setEdit({ ...edit, metadata_schema: v })} rows={5} placeholder='{"type":"object","required":["level"]}' />
          </Field>
          <div className="row">
            <Toggle value={!!edit.exclude_from_activity} onChange={(v) => setEdit({ ...edit, exclude_from_activity: v })} label="Exclude from activity (does not count towards last-seen / event totals in reports)" />
            <label className="field"><span className="label">Labels</span><input value={(edit.tags ?? []).join(', ')} onChange={(e) => setEdit({ ...edit, tags: e.target.value.split(',').map((s) => s.trim()).filter(Boolean) })} placeholder="economy, core-loop" /></label>
          </div>
          <div className="actions">
            <button className="btn" onClick={save} disabled={!edit.name}>Save</button>
            <button className="btn ghost" onClick={() => setEdit(null)}>Cancel</button>
            {edit.updated_at && !edit.is_core && (
              <Confirm danger onConfirm={() => run(async () => { await del(`/taxonomy/events/${encodeURIComponent(edit.name!)}`); setEdit(null); defs.reload(); })}>Delete</Confirm>
            )}
          </div>
        </div>
      )}
      <Table<EventDef>
        rows={rows}
        rowKey={(d) => d.name}
        onRow={(d) => setEdit({ ...d })}
        empty="No event definitions yet — they appear automatically as your game sends events"
        cols={[
          { h: 'Event', c: (d) => <><span className="mono">{d.name}</span> {d.is_core && <Badge tone="info">core</Badge>} {d.discovered && <Badge tone="warn">discovered</Badge>}</> },
          { h: 'Value', w: '100px', c: (d) => <Badge>{d.value_type}</Badge> },
          { h: 'Metadata schema', w: '140px', c: (d) => d.metadata_schema ? <Badge tone="ok">validated</Badge> : <span className="muted">—</span> },
          { h: 'Labels', c: (d) => d.tags.map((t) => <Badge key={t}>{t}</Badge>) },
          { h: 'Description', c: (d) => <span className="hint">{d.description}</span> },
        ]}
      />
      <h3 style={{ marginTop: 24 }}>Rejected & invalid events <span className="hint">(last {rejected.data?.length ?? 0})</span></h3>
      <p className="muted">Events that violated a definition. In <strong>strict</strong> mode they were dropped; in <strong>warn</strong> mode they were stored anyway.</p>
      <div className="actions"><button className="btn ghost" onClick={() => rejected.reload()}>Refresh</button><Confirm onConfirm={() => run(async () => { await del('/taxonomy/rejected'); rejected.reload(); })}>Clear</Confirm></div>
      <Table<Rejected>
        rows={rejected.data}
        rowKey={(r) => `${r.at}-${r.name}-${r.reason}`}
        empty="No violations recorded"
        cols={[
          { h: 'When', w: '180px', c: (r) => fmtDate(r.at) },
          { h: 'Event', c: (r) => <span className="mono">{r.name}</span> },
          { h: 'Player', c: (r) => <span className="mono hint">{r.external_id ?? '—'}</span> },
          { h: 'Value', c: (r) => <code className="hint trunc">{r.value ?? ''}</code> },
          { h: 'Reason', c: (r) => <span>{r.reason}</span> },
          { h: '', w: '90px', c: (r) => <Badge tone={r.rejected ? 'bad' : 'warn'}>{r.rejected ? 'dropped' : 'stored'}</Badge> },
        ]}
      />
    </>
  );
}

// ---- properties -------------------------------------------------------------------------------

function Properties() {
  const q = useLoad(() => get<{ config: PropCfg[]; default: Observed[]; computed: Observed[]; custom: Observed[] }>('/taxonomy/properties'));
  const [key, setKey] = useState('');
  const [desc, setDesc] = useState('');
  const { error, run } = useErr();
  const cfg = new Map((q.data?.config ?? []).map((c) => [c.key, c]));
  const toggleRo = (k: string, ro: boolean, description = '') => run(async () => { await put(`/taxonomy/properties/${encodeURIComponent(k)}`, { namespace: 'custom', read_only: ro, description }); q.reload(); });
  const section = (title: string, ns: string, rows: Observed[] | undefined, editable: boolean) => (
    <>
      <h3 style={{ marginTop: 20 }}>{title}</h3>
      <Table<Observed>
        rows={rows}
        rowKey={(r) => r.key}
        empty="Nothing observed yet"
        cols={[
          { h: 'Key', c: (r) => <span className="mono">{ns}.{r.key}</span> },
          { h: 'Type', w: '100px', c: (r) => <Badge>{r.json_type}</Badge> },
          { h: 'Players', w: '110px', c: (r) => fmtNum(r.identities) },
          { h: 'Access', w: '220px', c: (r) => editable
            ? <Toggle value={!!cfg.get(r.key)?.read_only} onChange={(v) => toggleRo(r.key, v, cfg.get(r.key)?.description)} label="read-only for clients" />
            : <span className="muted">{ns === 'computed' ? 'derived from events' : 'set by client / server'}</span> },
          { h: 'Description', c: (r) => <span className="hint">{cfg.get(r.key)?.description ?? ''}</span> },
        ]}
      />
    </>
  );
  return (
    <>
      <ErrorBox error={error ?? q.error} />
      <div className="card">
        <h3>Read-only custom property</h3>
        <p className="muted">Read-only properties can only be written through the server API (e.g. from Nakama); clients get a 400 when they try to set them.</p>
        <div className="row" style={{ alignItems: 'end' }}>
          <label className="field"><span className="label">Key</span><input className="mono" value={key} onChange={(e) => setKey(e.target.value)} placeholder="vip_tier" /></label>
          <label className="field grow"><span className="label">Description</span><input value={desc} onChange={(e) => setDesc(e.target.value)} /></label>
          <button className="btn" disabled={!key} onClick={() => { toggleRo(key, true, desc); setKey(''); setDesc(''); }}>Mark read-only</button>
        </div>
        {(q.data?.config ?? []).length > 0 && (
          <div style={{ marginTop: 10 }}>{q.data!.config.map((c) => (
            <Badge key={c.key} tone={c.read_only ? 'info' : 'muted'}>{c.namespace}.{c.key}{c.read_only ? ' · read-only' : ''} <a href="#" onClick={(e) => { e.preventDefault(); run(async () => { await del(`/taxonomy/properties/${encodeURIComponent(c.key)}`); q.reload(); }); }}>×</a></Badge>
          ))}</div>
        )}
      </div>
      {section('Default properties', 'default', q.data?.default, false)}
      {section('Computed properties', 'computed', q.data?.computed, false)}
      {section('Custom properties', 'custom', q.data?.custom, true)}
    </>
  );
}

// ---- schemas ----------------------------------------------------------------------------------

function Schemas() {
  const q = useLoad(() => get<Schema[]>('/schemas'));
  const [edit, setEdit] = useState<Partial<Schema> | null>(null);
  const [sample, setSample] = useState<unknown>({});
  const [verdict, setVerdict] = useState<string | null>(null);
  const { error, run } = useErr();
  const save = () => run(async () => {
    if (edit!.id) await put(`/schemas/${edit!.id}`, { name: edit!.name, description: edit!.description, schema: edit!.schema });
    else await post('/schemas', { name: edit!.name, description: edit!.description, schema: edit!.schema });
    setEdit(null); q.reload();
  });
  const test = () => run(async () => {
    const r = await post<{ results: (string | null)[] }>('/schemas/validate', { schema: edit!.schema, samples: [sample] });
    setVerdict(r.results[0] ?? 'valid');
  });
  return (
    <>
      <p className="muted">JSON Schemas describe the shape of structured flag and live-event values. Attach one to a flag and every variant, override and default is validated on save.</p>
      <div className="actions"><button className="btn" onClick={() => { setEdit({ name: '', description: '', schema: { type: 'object', properties: {} } }); setVerdict(null); }}>New schema</button></div>
      <ErrorBox error={error ?? q.error} />
      {edit && (
        <div className="card">
          <div className="row">
            <label className="field"><span className="label">Name</span><input value={edit.name ?? ''} onChange={(e) => setEdit({ ...edit, name: e.target.value })} placeholder="shop_config" /></label>
            <label className="field grow"><span className="label">Description</span><input value={edit.description ?? ''} onChange={(e) => setEdit({ ...edit, description: e.target.value })} /></label>
          </div>
          <div className="row">
            <div className="grow"><Field label="JSON Schema (draft 2020-12 / 7)"><JsonField value={edit.schema} onChange={(v) => setEdit({ ...edit, schema: v })} rows={14} /></Field></div>
            <div className="grow">
              <Field label="Try a value" hint={verdict ? <Badge tone={verdict === 'valid' ? 'ok' : 'bad'}>{verdict}</Badge> : 'Paste a sample value and validate'}>
                <JsonField value={sample} onChange={setSample} rows={10} />
              </Field>
              <button className="btn ghost" onClick={test}>Validate sample</button>
            </div>
          </div>
          <div className="actions">
            <button className="btn" onClick={save} disabled={!edit.name}>Save</button>
            <button className="btn ghost" onClick={() => setEdit(null)}>Cancel</button>
            {edit.id && <Confirm danger onConfirm={() => run(async () => { await del(`/schemas/${edit.id}`); setEdit(null); q.reload(); })}>Delete</Confirm>}
          </div>
        </div>
      )}
      <Table<Schema>
        rows={q.data}
        rowKey={(s) => s.id}
        onRow={(s) => { setEdit({ ...s }); setVerdict(null); }}
        empty="No schemas yet"
        cols={[
          { h: 'Name', c: (s) => <strong>{s.name}</strong> },
          { h: 'Description', c: (s) => <span className="hint">{s.description}</span> },
          { h: 'Updated', w: '180px', c: (s) => fmtDate(s.updated_at) },
        ]}
      />
    </>
  );
}

// ---- labels -----------------------------------------------------------------------------------

const COLORS = ['#6366f1', '#22c55e', '#f59e0b', '#ef4444', '#06b6d4', '#ec4899', '#84cc16', '#a855f7', '#64748b'];

function Labels() {
  const q = useLoad(() => get<Label[]>('/labels'));
  const [edit, setEdit] = useState<Partial<Label> | null>(null);
  const [usage, setUsage] = useState<Record<string, { table: string; count: number }[]>>({});
  const { error, run } = useErr();
  const save = () => run(async () => {
    const body = { name: edit!.name, color: edit!.color ?? '', description: edit!.description ?? '' };
    if (edit!.id) await put(`/labels/${edit!.id}`, body); else await post('/labels', body);
    setEdit(null); q.reload();
  });
  const loadUsage = (id: string) => run(async () => setUsage({ ...usage, [id]: await get(`/labels/${id}/usage`) }));
  return (
    <>
      <p className="muted">Labels (category tags) group flags, experiments, audiences, live events, messages and event definitions. Renaming a label updates every tagged item; clients can filter by label.</p>
      <div className="actions"><button className="btn" onClick={() => setEdit({ name: '', color: COLORS[0], description: '' })}>New label</button></div>
      <ErrorBox error={error ?? q.error} />
      {edit && (
        <div className="card">
          <div className="row" style={{ alignItems: 'end' }}>
            <label className="field"><span className="label">Name</span><input value={edit.name ?? ''} onChange={(e) => setEdit({ ...edit, name: e.target.value })} placeholder="season-5" /></label>
            <div className="field"><span className="label">Color</span>
              <div className="row" style={{ gap: 6 }}>{COLORS.map((c) => <button key={c} className="swatch" style={{ background: c, outline: edit.color === c ? '2px solid var(--fg)' : 'none' }} onClick={() => setEdit({ ...edit, color: c })} />)}</div>
            </div>
            <label className="field grow"><span className="label">Description</span><input value={edit.description ?? ''} onChange={(e) => setEdit({ ...edit, description: e.target.value })} /></label>
          </div>
          <div className="actions">
            <button className="btn" onClick={save} disabled={!edit.name}>Save</button>
            <button className="btn ghost" onClick={() => setEdit(null)}>Cancel</button>
            {edit.id && <Confirm danger onConfirm={() => run(async () => { await del(`/labels/${edit.id}`); setEdit(null); q.reload(); })}>Delete (removes it from every item)</Confirm>}
          </div>
        </div>
      )}
      <Table<Label>
        rows={q.data}
        rowKey={(l) => l.id}
        onRow={(l) => setEdit({ ...l })}
        empty="No labels yet"
        cols={[
          { h: 'Label', c: (l) => <span className="label-chip" style={{ background: l.color || '#64748b' }}>{l.name}</span> },
          { h: 'Description', c: (l) => <span className="hint">{l.description}</span> },
          { h: 'Used by', c: (l) => usage[l.id]
            ? usage[l.id].filter((u) => u.count > 0).map((u) => <Badge key={u.table}>{u.table}: {u.count}</Badge>)
            : <a href="#" onClick={(e) => { e.preventDefault(); e.stopPropagation(); loadUsage(l.id); }}>show usage</a> },
          { h: 'Created', w: '180px', c: (l) => fmtDate(l.created_at) },
        ]}
      />
    </>
  );
}
