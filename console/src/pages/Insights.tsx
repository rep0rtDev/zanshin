import { useState } from 'react';
import { del, get, post, qs, useLoad } from '../api';
import { EntityPage, Num, Tags, Text } from '../crud';
import { ErrorBox, Field, JsonField, Page, Stat, Table, fmtDate, fmtNum } from '../ui';

type Totals = Record<string, number>;
type Day = { day: string; dau: number; new_users: number; sessions: number; avg_session_secs: number; revenue: number; payers: number; purchases: number };
type Overview = { totals: Totals; series: Day[]; revenue_events: string[]; backend: string };
type Annotation = { id: string; at: string; kind: string; key: string; title: string; body: string };
type Cohort = { cohort: string; size: number; retained: Record<string, { count: number; rate: number } | null> };
type FunnelStep = { name: string; field?: string | null; op?: string | null; value?: unknown; count?: number; conversion?: number; step_conversion?: number; drop?: number };
type Widget = { id: string; kind: string; title: string; width: number; config: Record<string, unknown> };
type Dashboard = { id: string; key: string; name: string; description: string; widgets: Widget[]; filters: unknown; tags: string[]; version: number; updated_at: string };

const RANGES: [string, number][] = [['7d', 7], ['30d', 30], ['90d', 90]];
const pct = (x: number | undefined) => (x == null ? '—' : (x * 100).toFixed(1) + '%');
const money = (x: number) => x.toLocaleString(undefined, { maximumFractionDigits: 2 });
const since = (days: number) => new Date(Date.now() - days * 86400e3).toISOString();

function useRange() {
  const [days, setDays] = useState(30);
  const picker = <div className="chips">{RANGES.map(([l, d]) => <button key={l} className={'chip' + (d === days ? ' on' : '')} onClick={() => setDays(d)}>{l}</button>)}</div>;
  return { days, from: since(days), picker };
}

function Bars({ series, field, annotations, color = 'var(--fg)', money: isMoney }: { series: Day[]; field: keyof Day; annotations?: Annotation[]; color?: string; money?: boolean }) {
  const [hover, setHover] = useState<number | null>(null);
  const vals = series.map((d) => Number(d[field]) || 0);
  const max = Math.max(1, ...vals);
  const W = 600, H = 120;
  const bw = W / Math.max(1, vals.length);
  const marks = (annotations ?? []).map((a) => ({ a, i: series.findIndex((d) => d.day === a.at.slice(0, 10)) })).filter((m) => m.i >= 0);
  return (
    <div>
      <svg viewBox={`0 0 ${W} ${H + 16}`} style={{ width: '100%', height: 150 }} onMouseLeave={() => setHover(null)}>
        {vals.map((v, i) => <rect key={i} x={i * bw + 1} y={H - (v / max) * H} width={Math.max(1, bw - 2)} height={(v / max) * H} fill={color} opacity={hover === i ? 1 : 0.7} onMouseEnter={() => setHover(i)} />)}
        {marks.map((m, k) => <line key={k} x1={m.i * bw + bw / 2} x2={m.i * bw + bw / 2} y1={0} y2={H} stroke="#b26a00" strokeDasharray="3 3" />)}
        {series.length > 0 && <><text x={0} y={H + 12} fontSize={10} fill="var(--muted)">{series[0].day}</text><text x={W} y={H + 12} fontSize={10} fill="var(--muted)" textAnchor="end">{series[series.length - 1].day}</text></>}
      </svg>
      <div className="hint" style={{ minHeight: 18 }}>
        {hover != null && series[hover] ? <>{series[hover].day}: <strong>{isMoney ? money(vals[hover]) : fmtNum(vals[hover])}</strong>{marks.filter((m) => m.i === hover).map((m) => <span key={m.a.id}> · {m.a.title}</span>)}</> : marks.length ? `${marks.length} annotation(s) in range` : ''}
      </div>
    </div>
  );
}

function GameMetrics() {
  const r = useRange();
  const q = useLoad(() => get<Overview>('/analytics/overview' + qs({ from: r.from })), [r.from]);
  const ann = useLoad(() => get<{ items: Annotation[] }>('/analytics/annotations' + qs({ from: r.from })), [r.from]);
  const [note, setNote] = useState('');
  const t = q.data?.totals ?? {};
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'center' }}>{r.picker}<span className="hint">backend: {q.data?.backend ?? '…'} · revenue events: {q.data?.revenue_events.join(', ')}</span></div>
      <ErrorBox error={q.error ?? ann.error} />
      <div className="stats">
        <Stat label="DAU (latest)" value={fmtNum(t.dau ?? 0)} sub={`avg ${fmtNum(Math.round(t.dau_avg ?? 0))}`} />
        <Stat label="WAU / MAU" value={`${fmtNum(t.wau ?? 0)} / ${fmtNum(t.mau ?? 0)}`} sub={`stickiness ${pct(t.stickiness)}`} />
        <Stat label="New players" value={fmtNum(t.new_users ?? 0)} sub={`${fmtNum(t.total_players ?? 0)} total`} />
        <Stat label="Sessions" value={fmtNum(t.sessions ?? 0)} sub={`avg ${Math.round((t.avg_session_secs ?? 0) / 60)} min`} />
        <Stat label="Revenue" value={money(t.revenue ?? 0)} sub={`${fmtNum(t.purchases ?? 0)} purchases`} />
        <Stat label="ARPDAU / ARPPU" value={`${money(t.arpdau ?? 0)} / ${money(t.arppu ?? 0)}`} sub={`paying share ${pct(t.paying_share)}`} />
      </div>
      {q.data && <>
        <div className="card"><strong>Daily active players</strong><Bars series={q.data.series} field="dau" annotations={ann.data?.items} /></div>
        <div className="row">
          <div className="card" style={{ flex: 1 }}><strong>New players</strong><Bars series={q.data.series} field="new_users" color="#1e8e3e" annotations={ann.data?.items} /></div>
          <div className="card" style={{ flex: 1 }}><strong>Revenue</strong><Bars series={q.data.series} field="revenue" color="#1a5fb4" annotations={ann.data?.items} money /></div>
        </div>
      </>}
      <div className="card">
        <div className="row" style={{ alignItems: 'end' }}>
          <strong>Annotations</strong>
          <Text label="Add a note" value={note} onChange={setNote} placeholder="Soft launch in DE" />
          <button className="btn sm" disabled={!note} onClick={() => post('/analytics/annotations', { title: note }).then(() => { setNote(''); ann.reload(); })}>Add</button>
        </div>
        <Table<Annotation> rows={ann.data?.items} rowKey={(a) => a.id} empty="No changes in this range" cols={[
          { h: 'When', c: (a) => fmtDate(a.at) }, { h: 'Kind', c: (a) => <code>{a.kind}</code> }, { h: 'Title', c: (a) => a.title },
          { h: '', c: (a) => <button className="btn ghost sm" onClick={() => del(`/analytics/annotations/${a.id}`).then(ann.reload)}>Delete</button> },
        ]} />
      </div>
    </div>
  );
}

function Retention() {
  const r = useRange();
  const [days, setDays] = useState('1,3,7,14,30');
  const q = useLoad(() => get<{ days: number[]; cohorts: Cohort[] }>('/analytics/retention' + qs({ from: r.from, days })), [r.from, days]);
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'end' }}>{r.picker}<Text label="Days" value={days} onChange={setDays} mono hint="comma-separated, 1..90" /></div>
      <ErrorBox error={q.error} />
      <Table<Cohort> rows={q.data?.cohorts} rowKey={(c) => c.cohort} empty="No sign-ups in this range" cols={[
        { h: 'Cohort', c: (c) => <code>{c.cohort}</code> }, { h: 'Players', c: (c) => fmtNum(c.size) },
        ...(q.data?.days ?? []).map((d) => ({ h: `D${d}`, c: (c: Cohort) => { const v = c.retained[`d${d}`]; return v == null ? <span className="hint">—</span> : <span style={{ background: `rgba(26,95,180,${Math.min(1, v.rate + 0.05)})`, color: v.rate > 0.4 ? '#fff' : undefined, padding: '2px 6px', borderRadius: 4 }}>{pct(v.rate)}</span>; } })),
      ]} />
    </div>
  );
}

function StepsEditor({ value, onChange }: { value: FunnelStep[]; onChange: (v: FunnelStep[]) => void }) {
  const upd = (i: number, p: Partial<FunnelStep>) => onChange(value.map((s, j) => (j === i ? { ...s, ...p } : s)));
  return (
    <div className="stack">
      {value.map((s, i) => (
        <div key={i} className="row" style={{ alignItems: 'end' }}>
          <span className="hint" style={{ width: 20 }}>{i + 1}.</span>
          <Text label="Event" value={s.name} onChange={(name) => upd(i, { name })} mono />
          <Text label="Field (optional)" value={s.field ?? ''} onChange={(field) => upd(i, { field: field || null })} mono />
          <Field label="Op"><select value={s.op ?? 'eq'} onChange={(e) => upd(i, { op: e.target.value })}>{['eq', 'neq', 'exists', 'gt', 'gte', 'lt', 'lte', 'contains'].map((o) => <option key={o}>{o}</option>)}</select></Field>
          <Field label="Value (JSON)"><JsonField value={s.value ?? null} onChange={(value) => upd(i, { value })} rows={1} /></Field>
          <button className="btn ghost sm" disabled={value.length <= 1} onClick={() => onChange(value.filter((_, j) => j !== i))}>Remove</button>
        </div>
      ))}
      <div><button className="btn ghost sm" onClick={() => onChange([...value, { name: '' }])}>Add step</button></div>
    </div>
  );
}

function Funnels() {
  const r = useRange();
  const [steps, setSteps] = useState<FunnelStep[]>([{ name: '$session_start' }, { name: 'purchase' }]);
  const [windowSecs, setWindowSecs] = useState(7 * 86400);
  const [res, setRes] = useState<{ entered: number; steps: FunnelStep[]; truncated: boolean } | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const run = () => post<{ entered: number; steps: FunnelStep[]; truncated: boolean }>('/analytics/funnel', { steps, window_secs: windowSecs, from: r.from }).then((d) => { setRes(d); setErr(null); }, setErr);
  const max = Math.max(1, res?.entered ?? 1);
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'end' }}>{r.picker}<Num label="Conversion window (s)" value={windowSecs} onChange={(v) => setWindowSecs(v ?? 86400)} min={1} /><button className="btn" onClick={run} disabled={steps.some((s) => !s.name)}>Run</button></div>
      <StepsEditor value={steps} onChange={setSteps} />
      <ErrorBox error={err} />
      {res && (
        <div className="card">
          {res.truncated && <div className="hint">Result computed on a capped sample of events.</div>}
          {res.steps.map((s, i) => (
            <div key={i} className="row" style={{ alignItems: 'center', margin: '6px 0' }}>
              <span style={{ width: 180 }} className="mono">{i + 1}. {s.name}{s.field ? ` [${s.field} ${s.op} ${JSON.stringify(s.value)}]` : ''}</span>
              <div style={{ flex: 1, background: 'var(--border)', height: 18, borderRadius: 4 }}><div style={{ width: `${((s.count ?? 0) / max) * 100}%`, background: '#1a5fb4', height: '100%', borderRadius: 4 }} /></div>
              <span style={{ width: 220, textAlign: 'right' }}>{fmtNum(s.count ?? 0)} · {pct(s.conversion)} total{i > 0 && <span className="hint"> · {pct(s.step_conversion)} step, −{fmtNum(s.drop ?? 0)}</span>}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

const WKINDS: Record<string, string> = { number: 'Number', timeseries: 'Event timeseries', funnel: 'Funnel', retention: 'Retention', top_events: 'Top events' };
const METRICS = ['dau', 'dau_avg', 'wau', 'mau', 'new_users', 'total_players', 'sessions', 'avg_session_secs', 'revenue', 'purchases', 'arpdau', 'arppu', 'paying_share', 'stickiness', 'event_count'];

function WidgetEditor({ value, onChange }: { value: Widget[]; onChange: (v: Widget[]) => void }) {
  const upd = (i: number, p: Partial<Widget>) => onChange(value.map((w, j) => (j === i ? { ...w, ...p } : w)));
  const cfg = (i: number, p: Record<string, unknown>) => upd(i, { config: { ...value[i].config, ...p } });
  return (
    <div className="stack">
      {value.map((w, i) => (
        <div key={i} className="card" style={{ padding: 10 }}>
          <div className="row" style={{ alignItems: 'end' }}>
            <Text label="Id" value={w.id} onChange={(id) => upd(i, { id })} mono />
            <Field label="Kind"><select value={w.kind} onChange={(e) => upd(i, { kind: e.target.value, config: {} })}>{Object.keys(WKINDS).map((k) => <option key={k} value={k}>{WKINDS[k]}</option>)}</select></Field>
            <Text label="Title" value={w.title} onChange={(title) => upd(i, { title })} />
            <Num label="Width (1–4)" value={w.width} onChange={(width) => upd(i, { width: width ?? 2 })} min={1} max={4} />
            <button className="btn ghost sm" onClick={() => onChange(value.filter((_, j) => j !== i))}>Remove</button>
          </div>
          {w.kind === 'number' && <div className="row"><Field label="Metric"><select value={(w.config.metric as string) ?? 'dau'} onChange={(e) => cfg(i, { metric: e.target.value })}>{METRICS.map((m) => <option key={m}>{m}</option>)}</select></Field>{w.config.metric === 'event_count' && <Text label="Event" value={(w.config.event as string) ?? ''} onChange={(event) => cfg(i, { event })} mono />}</div>}
          {w.kind === 'timeseries' && <Text label="Event" value={(w.config.event as string) ?? ''} onChange={(event) => cfg(i, { event })} mono />}
          {w.kind === 'funnel' && <><StepsEditor value={(w.config.steps as FunnelStep[]) ?? [{ name: '' }]} onChange={(steps) => cfg(i, { steps })} /><Num label="Window (s)" value={(w.config.window_secs as number) ?? 604800} onChange={(v) => cfg(i, { window_secs: v })} min={1} /></>}
          {w.kind === 'retention' && <Text label="Days" value={((w.config.days as number[]) ?? [1, 7, 30]).join(',')} onChange={(v) => cfg(i, { days: v.split(',').map((x) => parseInt(x, 10)).filter((x) => !isNaN(x)) })} mono />}
          {w.kind === 'top_events' && <Num label="Limit" value={(w.config.limit as number) ?? 10} onChange={(limit) => cfg(i, { limit })} min={1} max={100} />}
        </div>
      ))}
      <div><button className="btn ghost sm" onClick={() => onChange([...value, { id: `w${value.length + 1}`, kind: 'number', title: '', width: 1, config: { metric: 'dau' } }])}>Add widget</button></div>
    </div>
  );
}

type WidgetData = { id: string; kind: string; title: string; width: number; data?: unknown; error?: string };
function WidgetView({ w }: { w: WidgetData }) {
  if (w.error) return <ErrorBox error={new Error(w.error)} />;
  const d = w.data as Record<string, unknown>;
  switch (w.kind) {
    case 'number': return <div className="big">{typeof d.value === 'number' ? (Number.isInteger(d.value) ? fmtNum(d.value) : money(d.value)) : String(d.value ?? '—')}</div>;
    case 'timeseries': { const b = (w.data as { bucket: string; count: number }[]); return <Bars series={b.map((x) => ({ day: x.bucket.slice(0, 10), dau: x.count } as unknown as Day))} field="dau" />; }
    case 'funnel': { const steps = d.steps as FunnelStep[]; const max = Math.max(1, Number(d.entered)); return <div>{steps.map((s, i) => <div key={i} className="row" style={{ alignItems: 'center' }}><span className="mono" style={{ width: 120 }}>{s.name}</span><div style={{ flex: 1, background: 'var(--border)', height: 12, borderRadius: 3 }}><div style={{ width: `${((s.count ?? 0) / max) * 100}%`, background: '#1a5fb4', height: '100%', borderRadius: 3 }} /></div><span style={{ width: 90, textAlign: 'right' }}>{fmtNum(s.count ?? 0)} · {pct(s.conversion)}</span></div>)}</div>; }
    case 'retention': { const cs = d.cohorts as Cohort[]; const days = d.days as number[]; return <Table<Cohort> rows={cs} rowKey={(c) => c.cohort} cols={[{ h: 'Cohort', c: (c) => c.cohort }, { h: 'N', c: (c) => c.size }, ...days.map((dd) => ({ h: `D${dd}`, c: (c: Cohort) => pct(c.retained[`d${dd}`]?.rate) }))]} />; }
    case 'top_events': return <Table<{ name: string; count: number; identities: number }> rows={w.data as { name: string; count: number; identities: number }[]} rowKey={(r) => r.name} cols={[{ h: 'Event', c: (r) => <code>{r.name}</code> }, { h: 'Count', c: (r) => fmtNum(r.count) }, { h: 'Players', c: (r) => fmtNum(r.identities) }]} />;
    default: return null;
  }
}

function DashboardView({ d }: { d: Dashboard }) {
  const r = useRange();
  const q = useLoad(() => get<{ widgets: WidgetData[] }>(`/dashboards/${encodeURIComponent(d.key)}/data` + qs({ from: r.from })), [d.key, d.version, r.from]);
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}><strong>Preview</strong>{r.picker}<button className="btn ghost sm" onClick={q.reload}>Refresh</button></div>
      <ErrorBox error={q.error} />
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: 10, marginTop: 8 }}>
        {(q.data?.widgets ?? []).map((w) => <div key={w.id} className="card" style={{ gridColumn: `span ${Math.min(4, Math.max(1, w.width))}` }}><div className="hint">{w.title || w.id}</div><WidgetView w={w} /></div>)}
      </div>
    </div>
  );
}

function Dashboards() {
  return (
    <EntityPage<Dashboard>
      title="Dashboards"
      path="/dashboards"
      blurb="Custom dashboards: numbers, event timeseries, funnels, retention and top-events widgets, evaluated for a time range."
      cols={[{ h: 'Key', c: (d) => <><strong>{d.key}</strong><div className="hint">{d.name}</div></> }, { h: 'Widgets', c: (d) => d.widgets.length }, { h: 'Updated', c: (d) => fmtDate(d.updated_at) }]}
      blank={() => ({ key: '', name: '', description: '', widgets: [{ id: 'dau', kind: 'number', title: 'DAU', width: 1, config: { metric: 'dau' } }], filters: {}, tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row"><Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono /><Text label="Name" value={d.name} onChange={(name) => set({ name })} /></div>
          <Text label="Description" value={d.description} onChange={(description) => set({ description })} />
          <Field label="Widgets"><WidgetEditor value={d.widgets ?? []} onChange={(widgets) => set({ widgets })} /></Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(d) => <DashboardView d={d} />}
    />
  );
}

export default function Insights() {
  const [tab, setTab] = useState<'metrics' | 'retention' | 'funnels' | 'dashboards'>('metrics');
  const tabs: [typeof tab, string][] = [['metrics', 'Game metrics'], ['retention', 'Retention'], ['funnels', 'Funnels'], ['dashboards', 'Dashboards']];
  if (tab === 'dashboards') return <><div className="chips" style={{ margin: '0 0 8px' }}>{tabs.map(([k, l]) => <button key={k} className={'chip' + (k === tab ? ' on' : '')} onClick={() => setTab(k)}>{l}</button>)}</div><Dashboards /></>;
  return (
    <Page title="Insights">
      <div className="chips" style={{ marginBottom: 12 }}>{tabs.map(([k, l]) => <button key={k} className={'chip' + (k === tab ? ' on' : '')} onClick={() => setTab(k)}>{l}</button>)}</div>
      {tab === 'metrics' && <GameMetrics />}
      {tab === 'retention' && <Retention />}
      {tab === 'funnels' && <Funnels />}
    </Page>
  );
}
