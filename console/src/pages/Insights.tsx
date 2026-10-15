import { useState } from 'react';
import { del, get, post, qs, useLoad } from '../api';
import { EntityPage, Num, Tags, Text } from '../crud';
import { ErrorBox, Field, JsonField, Page, Stat, Table, Toggle, fmtDate, fmtNum } from '../ui';
import { Calendar, Errors, Explore, FilterBar, Ltv, PlayerMap, Playtime, StatusBoard, filterQs, type Filter } from './InsightsV3';
import { t as tr } from '../i18n';

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

function GameMetrics({ filter }: { filter: Filter }) {
  const r = useRange();
  const q = useLoad(() => get<Overview>('/analytics/overview' + qs({ from: r.from, ...filterQs(filter) })), [r.from, JSON.stringify(filter)]);
  const ann = useLoad(() => get<{ items: Annotation[] }>('/analytics/annotations' + qs({ from: r.from })), [r.from]);
  const [note, setNote] = useState('');
  const t = q.data?.totals ?? {};
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'center' }}>{r.picker}<span className="hint">backend: {q.data?.backend ?? '…'} · revenue events: {q.data?.revenue_events.join(', ')}</span></div>
      <ErrorBox error={q.error ?? ann.error} />
      <div className="stats">
        <Stat label={tr('DAU (latest)')} value={fmtNum(t.dau ?? 0)} sub={`avg ${fmtNum(Math.round(t.dau_avg ?? 0))}`} />
        <Stat label={tr('WAU / MAU')} value={`${fmtNum(t.wau ?? 0)} / ${fmtNum(t.mau ?? 0)}`} sub={`stickiness ${pct(t.stickiness)}`} />
        <Stat label={tr('New players')} value={fmtNum(t.new_users ?? 0)} sub={`${fmtNum(t.total_players ?? 0)} total`} />
        <Stat label={tr('Sessions')} value={fmtNum(t.sessions ?? 0)} sub={`avg ${Math.round((t.avg_session_secs ?? 0) / 60)} min`} />
        <Stat label={tr('Revenue')} value={money(t.revenue ?? 0)} sub={`${fmtNum(t.purchases ?? 0)} purchases`} />
        <Stat label={tr('ARPDAU / ARPPU')} value={`${money(t.arpdau ?? 0)} / ${money(t.arppu ?? 0)}`} sub={`paying share ${pct(t.paying_share)}`} />
      </div>
      {q.data && <>
        <div className="card"><strong>{tr('Daily active players')}</strong><Bars series={q.data.series} field="dau" annotations={ann.data?.items} /></div>
        <div className="row">
          <div className="card" style={{ flex: 1 }}><strong>{tr('New players')}</strong><Bars series={q.data.series} field="new_users" color="#1e8e3e" annotations={ann.data?.items} /></div>
          <div className="card" style={{ flex: 1 }}><strong>{tr('Revenue')}</strong><Bars series={q.data.series} field="revenue" color="#1a5fb4" annotations={ann.data?.items} money /></div>
        </div>
      </>}
      <div className="card">
        <div className="row" style={{ alignItems: 'end' }}>
          <strong>{tr('Annotations')}</strong>
          <Text label={tr('Add a note')} value={note} onChange={setNote} placeholder={tr('Soft launch in DE')} />
          <button className="btn sm" disabled={!note} onClick={() => post('/analytics/annotations', { title: note }).then(() => { setNote(''); ann.reload(); })}>{tr('Add')}</button>
        </div>
        <Table<Annotation> rows={ann.data?.items} rowKey={(a) => a.id} empty={tr('No changes in this range')} cols={[
          { h: 'When', c: (a) => fmtDate(a.at) }, { h: 'Kind', c: (a) => <code>{a.kind}</code> }, { h: 'Title', c: (a) => a.title },
          { h: '', c: (a) => <button className="btn ghost sm" onClick={() => del(`/analytics/annotations/${a.id}`).then(ann.reload)}>{tr('Delete')}</button> },
        ]} />
      </div>
    </div>
  );
}

function Retention({ filter }: { filter: Filter }) {
  const r = useRange();
  const [days, setDays] = useState('1,3,7,14,30');
  const [cumulative, setCumulative] = useState(false);
  const [opKind, setOpKind] = useState<'' | 'live' | 'experiment'>('');
  const [opKey, setOpKey] = useState('');
  const [opVariant, setOpVariant] = useState('');
  const opFilter = opKind && opKey ? { ...filter, experiment: opKind === 'live' ? `live:${opKey}` : opKey, variant: opVariant || undefined } : filter;
  const q = useLoad(() => get<{ days: number[]; cohorts: Cohort[]; installs: number }>('/analytics/retention' + qs({ from: r.from, days, cumulative, ...filterQs(opFilter) })), [r.from, days, cumulative, opKind, opKey, opVariant, JSON.stringify(filter)]);
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'end' }}>{r.picker}<Text label={tr('Days')} value={days} onChange={setDays} mono hint={tr('comma-separated, 1..90')} /><Field label={tr('Operation')} hint={tr('retention of participants of a live event or experiment')}><select value={opKind} onChange={(e) => setOpKind(e.target.value as typeof opKind)}><option value="">{tr('all players')}</option><option value="live">{tr('live event')}</option><option value="experiment">experiment</option></select></Field>{opKind && <Text label={tr('Key')} value={opKey} onChange={setOpKey} mono />}{opKind && <Text label={tr('Variant')} value={opVariant} onChange={setOpVariant} mono hint="optional" />}<Field label={tr('Model')}><Toggle value={cumulative} onChange={setCumulative} label={cumulative ? 'cumulative — returned by day N' : 'classic — returned on day N'} /></Field><span className="hint">{fmtNum(q.data?.installs ?? 0)} installs in range</span></div>
      <ErrorBox error={q.error} />
      <Table<Cohort> rows={q.data?.cohorts} rowKey={(c) => c.cohort} empty={tr('No sign-ups in this range')} cols={[
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
          <Text label={tr('Event')} value={s.name} onChange={(name) => upd(i, { name })} mono />
          <Text label={tr('Field (optional)')} value={s.field ?? ''} onChange={(field) => upd(i, { field: field || null })} mono />
          <Field label={tr('Op')}><select value={s.op ?? 'eq'} onChange={(e) => upd(i, { op: e.target.value })}>{['eq', 'neq', 'exists', 'gt', 'gte', 'lt', 'lte', 'contains'].map((o) => <option key={o}>{o}</option>)}</select></Field>
          <Field label={tr('Value (JSON)')}><JsonField value={s.value ?? null} onChange={(value) => upd(i, { value })} rows={1} /></Field>
          <button className="btn ghost sm" disabled={value.length <= 1} onClick={() => onChange(value.filter((_, j) => j !== i))}>{tr('Remove')}</button>
        </div>
      ))}
      <div><button className="btn ghost sm" onClick={() => onChange([...value, { name: '' }])}>{tr('Add step')}</button></div>
    </div>
  );
}

type LifetimeRes = { players: number; truncated: boolean; averages: Record<string, number>; buckets: Record<string, { label: string; players: number }[]> };

function Lifetime() {
  const r = useRange();
  const q = useLoad(() => get<LifetimeRes>('/analytics/lifetime' + qs({ from: r.from })), [r.from]);
  const a = q.data?.averages ?? {};
  const dur = (s: number) => (s >= 3600 ? `${(s / 3600).toFixed(1)} h` : `${Math.round(s / 60)} min`);
  const Buckets = ({ title, rows }: { title: string; rows?: { label: string; players: number }[] }) => {
    const max = Math.max(1, ...(rows ?? []).map((b) => b.players));
    return (
      <div className="card stack" style={{ flex: 1, minWidth: 220 }}>
        <strong>{title}</strong>
        {(rows ?? []).map((b) => (
          <div key={b.label} className="row" style={{ alignItems: 'center' }}>
            <span className="mono" style={{ width: 70 }}>{b.label}</span>
            <div style={{ flex: 1, background: 'var(--border)', height: 14, borderRadius: 4 }}><div style={{ width: `${(b.players / max) * 100}%`, background: '#1a5fb4', height: '100%', borderRadius: 4 }} /></div>
            <span style={{ width: 70, textAlign: 'right' }}>{fmtNum(b.players)}</span>
          </div>
        ))}
      </div>
    );
  };
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'end' }}>{r.picker}<span className="hint">{tr('Players who installed in the range · lifetime figures from computed properties')}</span></div>
      <ErrorBox error={q.error} />
      <div className="stats">
        <Stat label={tr('Players')} value={fmtNum(q.data?.players ?? 0)} />
        <Stat label={tr('Avg sessions')} value={(a.sessions ?? 0).toFixed(1)} />
        <Stat label={tr('Avg play time')} value={dur(a.play_time_sec ?? 0)} />
        <Stat label={tr('Avg lifetime')} value={`${(a.lifetime_days ?? 0).toFixed(1)} d`} />
        <Stat label={tr('Payers')} value={pct((a.payers_pct ?? 0) / 100)} />
        <Stat label={tr('Avg spend')} value={money(a.spend_usd ?? 0)} />
        <Stat label={tr('ARPPU')} value={money(a.arppu_usd ?? 0)} />
      </div>
      {q.data?.truncated && <div className="hint">{tr('Computed on the first 200k players of the range.')}</div>}
      <div className="row" style={{ alignItems: 'stretch', flexWrap: 'wrap' }}>
        <Buckets title={tr('Sessions')} rows={q.data?.buckets.sessions} />
        <Buckets title={tr('Play time')} rows={q.data?.buckets.play_time} />
        <Buckets title={tr('Lifetime')} rows={q.data?.buckets.lifetime_days} />
        <Buckets title={tr('Spend (USD)')} rows={q.data?.buckets.spend_usd} />
      </div>
    </div>
  );
}

function Funnels({ filter }: { filter: Filter }) {
  const r = useRange();
  const [model, setModel] = useState<'unlimited' | 'window' | 'step' | 'session'>('window');
  const [count, setCount] = useState<'unique' | 'total'>('unique');
  const [cohort, setCohort] = useState<'static' | 'shifting'>('static');
  const [steps, setSteps] = useState<FunnelStep[]>([{ name: '$session_start' }, { name: 'purchase' }]);
  const [windowSecs, setWindowSecs] = useState(7 * 86400);
  const [res, setRes] = useState<{ entered: number; steps: FunnelStep[]; truncated: boolean } | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const run = () => post<{ entered: number; steps: FunnelStep[]; truncated: boolean }>('/analytics/funnel', { steps, window_secs: windowSecs, from: r.from, model, count, cohort, filter }).then((d) => { setRes(d); setErr(null); }, setErr);
  const max = Math.max(1, res?.entered ?? 1);
  const [cohortKey, setCohortKey] = useState('');
  const [cohortStep, setCohortStep] = useState(1);
  const [cohortReached, setCohortReached] = useState(true);
  const [cohortMsg, setCohortMsg] = useState('');
  const toAudience = () => post<{ audience: string; matched: number; members: number }>('/analytics/funnel/cohort', { steps, window_secs: windowSecs, from: r.from, model, count, cohort, filter, step: cohortStep, reached: cohortReached, audience_key: cohortKey }).then((d) => { setCohortMsg(`${d.matched} players → audience ${d.audience} (${d.members} members)`); setErr(null); }, setErr);
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'end' }}>{r.picker}<Field label={tr('Count')} hint={tr('unique: once per player · total: every entry')}><select value={count} onChange={(e) => setCount(e.target.value as typeof count)}><option value="unique">{tr('unique players')}</option><option value="total">{tr('total entries')}</option></select></Field><Field label={tr('Cohort')} hint={tr('static: entered in range · shifting: conversions in range, entry may be earlier')}><select value={cohort} onChange={(e) => setCohort(e.target.value as typeof cohort)}><option value="static">static</option><option value="shifting">shifting</option></select></Field><Field label={tr('Model')}><select value={model} onChange={(e) => setModel(e.target.value as typeof model)}><option value="window">{tr('window — all steps within N s of the first')}</option><option value="step">{tr('step — each step within N s of the previous')}</option><option value="session">{tr('session — all steps in one play session')}</option><option value="unlimited">{tr('unlimited — any time in range')}</option></select></Field>{model !== 'session' && model !== 'unlimited' && <Num label={tr('Window (s)')} value={windowSecs} onChange={(v) => setWindowSecs(v ?? 86400)} min={1} />}<button className="btn" onClick={run} disabled={steps.some((s) => !s.name)}>{tr('Run')}</button></div>
      <StepsEditor value={steps} onChange={setSteps} />
      <ErrorBox error={err} />
      {res && (
        <div className="card">
          {res.truncated && <div className="hint">{tr('Result computed on a capped sample of events.')}</div>}
          {res.steps.map((s, i) => (
            <div key={i} className="row" style={{ alignItems: 'center', margin: '6px 0' }}>
              <span style={{ width: 180 }} className="mono">{i + 1}. {s.name}{s.field ? ` [${s.field} ${s.op} ${JSON.stringify(s.value)}]` : ''}</span>
              <div style={{ flex: 1, background: 'var(--border)', height: 18, borderRadius: 4 }}><div style={{ width: `${((s.count ?? 0) / max) * 100}%`, background: '#1a5fb4', height: '100%', borderRadius: 4 }} /></div>
              <span style={{ width: 220, textAlign: 'right' }}>{fmtNum(s.count ?? 0)} · {pct(s.conversion)} total{i > 0 && <span className="hint"> · {pct(s.step_conversion)} step, −{fmtNum(s.drop ?? 0)}</span>}</span>
            </div>
          ))}
          <div className="row" style={{ alignItems: 'end', marginTop: 10 }}>
            <Field label={tr('Step')}><select value={cohortStep} onChange={(e) => setCohortStep(parseInt(e.target.value, 10))}>{res.steps.map((_, i) => <option key={i} value={i + 1}>{i + 1}</option>)}</select></Field>
            <Field label={tr('Who')}><select value={cohortReached ? 'reached' : 'dropped'} onChange={(e) => setCohortReached(e.target.value === 'reached')}><option value="reached">{tr('reached the step')}</option><option value="dropped">{tr('dropped before it')}</option></select></Field>
            <Text label={tr('Audience key')} value={cohortKey} onChange={setCohortKey} mono />
            <button className="btn ghost sm" disabled={!cohortKey} onClick={toAudience}>{tr('Save as audience')}</button>
            {cohortMsg && <span className="hint">{cohortMsg}</span>}
          </div>
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
            <Text label={tr('Id')} value={w.id} onChange={(id) => upd(i, { id })} mono />
            <Field label={tr('Kind')}><select value={w.kind} onChange={(e) => upd(i, { kind: e.target.value, config: {} })}>{Object.keys(WKINDS).map((k) => <option key={k} value={k}>{WKINDS[k]}</option>)}</select></Field>
            <Text label={tr('Title')} value={w.title} onChange={(title) => upd(i, { title })} />
            <Num label={tr('Width (1–4)')} value={w.width} onChange={(width) => upd(i, { width: width ?? 2 })} min={1} max={4} />
            <button className="btn ghost sm" onClick={() => onChange(value.filter((_, j) => j !== i))}>{tr('Remove')}</button>
          </div>
          {w.kind === 'number' && <div className="row"><Field label={tr('Metric')}><select value={(w.config.metric as string) ?? 'dau'} onChange={(e) => cfg(i, { metric: e.target.value })}>{METRICS.map((m) => <option key={m}>{m}</option>)}</select></Field>{w.config.metric === 'event_count' && <Text label={tr('Event')} value={(w.config.event as string) ?? ''} onChange={(event) => cfg(i, { event })} mono />}</div>}
          {w.kind === 'timeseries' && <Text label={tr('Event')} value={(w.config.event as string) ?? ''} onChange={(event) => cfg(i, { event })} mono />}
          {w.kind === 'funnel' && <><StepsEditor value={(w.config.steps as FunnelStep[]) ?? [{ name: '' }]} onChange={(steps) => cfg(i, { steps })} /><Num label={tr('Window (s)')} value={(w.config.window_secs as number) ?? 604800} onChange={(v) => cfg(i, { window_secs: v })} min={1} /></>}
          {w.kind === 'retention' && <Text label={tr('Days')} value={((w.config.days as number[]) ?? [1, 7, 30]).join(',')} onChange={(v) => cfg(i, { days: v.split(',').map((x) => parseInt(x, 10)).filter((x) => !isNaN(x)) })} mono />}
          {w.kind === 'top_events' && <Num label={tr('Limit')} value={(w.config.limit as number) ?? 10} onChange={(limit) => cfg(i, { limit })} min={1} max={100} />}
        </div>
      ))}
      <div><button className="btn ghost sm" onClick={() => onChange([...value, { id: `w${value.length + 1}`, kind: 'number', title: '', width: 1, config: { metric: 'dau' } }])}>{tr('Add widget')}</button></div>
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
      <div className="row" style={{ alignItems: 'center' }}><strong>{tr('Preview')}</strong>{r.picker}<button className="btn ghost sm" onClick={q.reload}>{tr('Refresh')}</button></div>
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
      title={tr('Dashboards')}
      path="/dashboards"
      blurb="Custom dashboards: numbers, event timeseries, funnels, retention and top-events widgets, evaluated for a time range."
      cols={[{ h: 'Key', c: (d) => <><strong>{d.key}</strong><div className="hint">{d.name}</div></> }, { h: 'Widgets', c: (d) => d.widgets.length }, { h: 'Updated', c: (d) => fmtDate(d.updated_at) }]}
      blank={() => ({ key: '', name: '', description: '', widgets: [{ id: 'dau', kind: 'number', title: 'DAU', width: 1, config: { metric: 'dau' } }], filters: {}, tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row"><Text label={tr('Key')} value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono /><Text label={tr('Name')} value={d.name} onChange={(name) => set({ name })} /></div>
          <Text label={tr('Description')} value={d.description} onChange={(description) => set({ description })} />
          <Field label={tr('Widgets')}><WidgetEditor value={d.widgets ?? []} onChange={(widgets) => set({ widgets })} /></Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(d) => <DashboardView d={d} />}
    />
  );
}

type Tab = 'status' | 'calendar' | 'metrics' | 'explore' | 'lifetime' | 'map' | 'playtime' | 'retention' | 'funnels' | 'ltv' | 'errors' | 'dashboards';
const FILTERED: Tab[] = ['metrics', 'explore', 'map', 'playtime', 'retention', 'funnels', 'ltv'];

export default function Insights() {
  const [tab, setTab] = useState<Tab>(() => (localStorage.getItem('zanshin.insights.tab') as Tab) || 'status');
  const [filter, setFilter] = useState<Filter>(() => { try { return JSON.parse(localStorage.getItem('zanshin.insights.filter') || '{}'); } catch { return {}; } });
  const pick = (t: Tab) => { setTab(t); localStorage.setItem('zanshin.insights.tab', t); };
  const setF = (f: Filter) => { setFilter(f); localStorage.setItem('zanshin.insights.filter', JSON.stringify(f)); };
  const tabs: [Tab, string][] = [['status', 'Status'], ['calendar', 'Calendar'], ['metrics', 'Game metrics'], ['explore', 'Explore'], ['lifetime', 'Lifetime'], ['map', 'Map'], ['playtime', 'Playtime'], ['retention', 'Retention'], ['funnels', 'Funnels'], ['ltv', 'LTV / RoAS'], ['errors', 'Errors'], ['dashboards', 'Dashboards']];
  const chips = <div className="chips" style={{ marginBottom: 12 }}>{tabs.map(([k, l]) => <button key={k} className={'chip' + (k === tab ? ' on' : '')} onClick={() => pick(k)}>{l}</button>)}</div>;
  if (tab === 'dashboards') return <>{chips}<Dashboards /></>;
  return (
    <Page title={tr('Insights')}>
      {chips}
      {FILTERED.includes(tab) && <div className="card" style={{ padding: '8px 12px', marginBottom: 12 }}><FilterBar value={filter} onChange={setF} /></div>}
      {tab === 'status' && <StatusBoard />}
      {tab === 'calendar' && <Calendar />}
      {tab === 'metrics' && <GameMetrics filter={filter} />}
      {tab === 'explore' && <Explore filter={filter} />}
      {tab === 'lifetime' && <Lifetime />}
      {tab === 'map' && <PlayerMap filter={filter} />}
      {tab === 'playtime' && <Playtime filter={filter} />}
      {tab === 'retention' && <Retention filter={filter} />}
      {tab === 'funnels' && <Funnels filter={filter} />}
      {tab === 'ltv' && <Ltv filter={filter} />}
      {tab === 'errors' && <Errors />}
    </Page>
  );
}
