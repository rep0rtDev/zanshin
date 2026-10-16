import { useState } from 'react';
import { Link } from 'react-router-dom';
import { get, post, qs, useLoad } from '../api';
import { Num, Text } from '../crud';
import { Badge, ErrorBox, Field, Stat, Table, fmtDate, fmtNum } from '../ui';
import { t as tr } from '../i18n';

export type Filter = { country?: string; platform?: string; app_version?: string; experiment?: string; variant?: string };
export const filterQs = (f: Filter) => Object.fromEntries(Object.entries(f).filter(([, v]) => v));
const pct = (x: number | null | undefined) => (x == null ? '—' : (x * 100).toFixed(1) + '%');
const money = (x: number | null | undefined) => (x == null ? '—' : x.toLocaleString(undefined, { maximumFractionDigits: 2 }));
const mins = (s: number) => `${Math.round(s / 60)} min`;
const since = (days: number) => new Date(Date.now() - days * 86400e3).toISOString();

/** Shared dimension filter for every event-based chart. */
export function FilterBar({ value, onChange }: { value: Filter; onChange: (f: Filter) => void }) {
  const set = (k: keyof Filter) => (v: string) => onChange({ ...value, [k]: v || undefined });
  return (
    <div className="row filter-bar" style={{ alignItems: 'end' }}>
      <Text label={tr('Country')} value={value.country ?? ''} onChange={set('country')} placeholder={tr('DE')} mono />
      <Text label={tr('Platform')} value={value.platform ?? ''} onChange={set('platform')} placeholder="ios" mono />
      <Text label={tr('App version')} value={value.app_version ?? ''} onChange={set('app_version')} placeholder="1.4.2" mono />
      <Text label={tr('Experiment')} value={value.experiment ?? ''} onChange={set('experiment')} placeholder="key" mono />
      <Text label={tr('Variant')} value={value.variant ?? ''} onChange={set('variant')} placeholder="b" mono />
      {Object.values(value).some(Boolean) && <button className="btn ghost sm" onClick={() => onChange({})}>{tr('Clear')}</button>}
    </div>
  );
}

export function useRangeDays(initial = 30) {
  const RANGES: [string, number][] = [['7d', 7], ['30d', 30], ['90d', 90]];
  const [days, setDays] = useState(initial);
  const picker = <div className="chips">{RANGES.map(([l, d]) => <button key={l} className={'chip' + (d === days ? ' on' : '')} onClick={() => setDays(d)}>{l}</button>)}</div>;
  return { days, from: since(days), picker };
}

type StatusData = {
  now: string; online_now: number; dau_today: number; events_last_hour: number; active_last_hour: number;
  running: { experiments: { key: string; name: string; status: string; ends_at: string | null }[]; live_events: { key: string; name: string; start: string; end: string }[]; campaigns: { key: string; name: string }[]; journeys: { key: string; name: string }[] };
  upcoming: { live_events: { key: string; name: string; start: string; end: string }[] };
  errors_24h: { rejected_events: number; failures: { source: string; count: number }[] };
};

const MAX_ROWS = 8;
const fmtShort = (iso: string) => new Date(iso).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
function MoreLink({ n, to }: { n: number; to: string }) {
  return n > MAX_ROWS ? <div className="hint" style={{ marginTop: 4 }}><Link to={to}>+{n - MAX_ROWS} more</Link></div> : null;
}

export function StatusBoard() {
  const q = useLoad(() => get<StatusData>('/analytics/status'));
  const d = q.data;
  const failures = d?.errors_24h.failures.reduce((a, f) => a + f.count, 0) ?? 0;
  return (
    <div className="stack">
      <ErrorBox error={q.error} />
      <div className="stats">
        <Stat label={tr('Online now')} value={fmtNum(d?.online_now ?? 0)} sub="open play sessions, last 5 min" />
        <Stat label={tr('DAU today')} value={fmtNum(d?.dau_today ?? 0)} sub={`${fmtNum(d?.active_last_hour ?? 0)} active last hour`} />
        <Stat label={tr('Events / min')} value={fmtNum(Math.round((d?.events_last_hour ?? 0) / 60))} sub={`${fmtNum(d?.events_last_hour ?? 0)} in the last hour`} />
        <Stat label={tr('Errors (24 h)')} value={<span style={{ color: (d?.errors_24h.rejected_events ?? 0) + failures > 0 ? 'var(--bad)' : undefined }}>{fmtNum((d?.errors_24h.rejected_events ?? 0) + failures)}</span>} sub={`${d?.errors_24h.rejected_events ?? 0} rejected events · ${failures} failed deliveries`} />
      </div>
      <div className="grid" style={{ gridTemplateColumns: 'repeat(3,minmax(0,1fr))' }}>
        <div className="card">
          <strong>{tr('Live now')}</strong>
          {d && d.running.live_events.length === 0 && <div className="hint">{tr('No live event is running.')}</div>}
          {d?.running.live_events.slice(0, MAX_ROWS).map((e) => <div key={e.key} className="list-row"><Link to={`/live-events/${e.key}`}>{e.name || e.key}</Link><span className="hint">until {fmtShort(e.end)}</span></div>)}
          <MoreLink n={d?.running.live_events.length ?? 0} to="/live-events" />
          <div className="hint" style={{ marginTop: 8 }}>{tr('Next 7 days')}</div>
          {d?.upcoming.live_events.slice(0, MAX_ROWS).map((e) => <div key={e.key + e.start} className="list-row"><Link to={`/live-events/${e.key}`}>{e.name || e.key}</Link><span className="hint">{fmtShort(e.start)}</span></div>)}
          <MoreLink n={d?.upcoming.live_events.length ?? 0} to="/insights?tab=calendar" />
        </div>
        <div className="card">
          <strong>{tr('Experiments')}</strong>
          {d && d.running.experiments.length === 0 && <div className="hint">{tr('Nothing running.')}</div>}
          {d?.running.experiments.slice(0, MAX_ROWS).map((e) => <div key={e.key} className="list-row"><Link to={`/experiments/${e.key}`}>{e.name || e.key}</Link><span><Badge tone={e.status === 'running' ? 'ok' : 'warn'}>{e.status}</Badge>{e.ends_at && <span className="hint"> ends {fmtShort(e.ends_at)}</span>}</span></div>)}
          <MoreLink n={d?.running.experiments.length ?? 0} to="/experiments" />
        </div>
        <div className="card">
          <strong>{tr('Campaigns & journeys')}</strong>
          {d?.running.campaigns.slice(0, MAX_ROWS).map((c) => <div key={c.key} className="list-row"><Link to={`/messages/${c.key}`}>{c.name || c.key}</Link><Badge>campaign</Badge></div>)}
          <MoreLink n={d?.running.campaigns.length ?? 0} to="/messages" />
          {d?.running.journeys.slice(0, MAX_ROWS).map((j) => <div key={j.key} className="list-row"><Link to={`/journeys/${j.key}`}>{j.name || j.key}</Link><Badge tone="info">journey</Badge></div>)}
          <MoreLink n={d?.running.journeys.length ?? 0} to="/journeys" />
          {d && d.running.campaigns.length + d.running.journeys.length === 0 && <div className="hint">{tr('Nothing enabled.')}</div>}
        </div>
      </div>
    </div>
  );
}

type CalItem = { kind: string; key: string; name: string; start: string; end: string | null; enabled?: boolean; status?: string; open_ended?: boolean };
type CalDay = { day: string; dau: number; revenue: number };
const KIND_COLOR: Record<string, string> = { live_event: '#b26a00', experiment: '#1a5fb4', campaign: '#1e8e3e', annotation: '#888' };

export function Calendar() {
  const [weeks, setWeeks] = useState(2);
  const [offset, setOffset] = useState(0);
  const start = new Date(); start.setUTCHours(0, 0, 0, 0); start.setUTCDate(start.getUTCDate() - start.getUTCDay() + 1 + offset * 7 * weeks);
  const end = new Date(start.getTime() + weeks * 7 * 86400e3);
  const q = useLoad(() => get<{ items: CalItem[]; metrics: CalDay[] }>('/analytics/calendar' + qs({ from: start.toISOString(), to: end.toISOString() })), [start.getTime(), end.getTime()]);
  const days = Array.from({ length: weeks * 7 }, (_, i) => new Date(start.getTime() + i * 86400e3));
  const span = end.getTime() - start.getTime();
  const pos = (iso: string) => Math.min(1, Math.max(0, (new Date(iso).getTime() - start.getTime()) / span));
  const lanes: Record<string, CalItem[]> = {};
  for (const it of q.data?.items ?? []) (lanes[it.kind] ??= []).push(it);
  const maxDau = Math.max(1, ...(q.data?.metrics ?? []).map((m) => m.dau));
  const todayPos = pos(new Date().toISOString());
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'center' }}>
        <div className="chips">{[1, 2, 4].map((w) => <button key={w} className={'chip' + (w === weeks ? ' on' : '')} onClick={() => { setWeeks(w); setOffset(0); }}>{w} wk</button>)}</div>
        <button className="btn ghost sm" onClick={() => setOffset(offset - 1)}>‹</button>
        <strong>{start.toISOString().slice(0, 10)} → {new Date(end.getTime() - 1).toISOString().slice(0, 10)}</strong>
        <button className="btn ghost sm" onClick={() => setOffset(offset + 1)}>›</button>
        {offset !== 0 && <button className="btn ghost sm" onClick={() => setOffset(0)}>{tr('Today')}</button>}
        <span className="hint">{tr('live events · experiments · campaigns (cron fire windows) · annotations, DAU overlay')}</span>
      </div>
      <ErrorBox error={q.error} />
      <div className="card calendar">
        <div className="cal-days" style={{ gridTemplateColumns: `repeat(${days.length}, 1fr)` }}>
          {days.map((d) => <div key={d.toISOString()} className={'cal-day' + (d.toISOString().slice(0, 10) === new Date().toISOString().slice(0, 10) ? ' today' : '')}>{weeks === 4 ? d.getUTCDate() : d.toISOString().slice(5, 10)}</div>)}
        </div>
        <div className="cal-metrics" style={{ gridTemplateColumns: `repeat(${days.length}, 1fr)` }}>
          {days.map((d) => { const m = q.data?.metrics.find((x) => x.day === d.toISOString().slice(0, 10)); const h = m ? (m.dau / maxDau) * 100 : 0; return <div key={d.toISOString()} className="cal-bar" title={m ? `${m.day}: DAU ${fmtNum(m.dau)} · revenue ${money(m.revenue)}` : ''}><div style={{ height: `${h}%` }} /></div>; })}
        </div>
        {(['live_event', 'experiment', 'campaign', 'annotation'] as const).map((kind) => lanes[kind] && (
          <div key={kind} className="cal-lane">
            <div className="cal-lane-title">{kind.replace('_', ' ')}</div>
            <div className="cal-lane-body">
              {lanes[kind].map((it, i) => {
                const l = pos(it.start), r = it.end ? pos(it.end) : 1;
                const w = Math.max(0.6, (r - l) * 100);
                const to = kind === 'live_event' ? `/live-events/${it.key}` : kind === 'experiment' ? `/experiments/${it.key}` : kind === 'campaign' ? `/messages/${it.key}` : '#';
                return <Link key={i} to={to} className="cal-item" title={`${it.name}\n${fmtDate(it.start)}${it.end ? ' → ' + fmtDate(it.end) : ''}`} style={{ left: `${l * 100}%`, width: `${w}%`, background: KIND_COLOR[kind], opacity: it.enabled === false ? 0.4 : 1, top: (i % 3) * 20 }}>{it.name || it.key}{it.open_ended ? ' →' : ''}</Link>;
              })}
            </div>
          </div>
        ))}
        {q.data && q.data.items.length === 0 && <div className="hint" style={{ padding: 12 }}>{tr('Nothing scheduled in this range.')}</div>}
        <div className="cal-today" style={{ left: `calc(120px + (100% - 120px) * ${todayPos})` }} />
      </div>
    </div>
  );
}

type ExploreRes = { total: { count: number; sum: number; field_sum: number }; groups: { group: string; count: number; identities: number; sum: number; field_sum: number }[]; series: { bucket: string; group: string; count: number }[]; granularity: string };

export function Explore({ filter }: { filter: Filter }) {
  const r = useRangeDays();
  const [event, setEvent] = useState('$session_start');
  const [groupBy, setGroupBy] = useState('');
  const [sumField, setSumField] = useState('amount');
  const [gran, setGran] = useState<'day' | 'hour'>('day');
  const [res, setRes] = useState<ExploreRes | null>(null);
  const [overlay, setOverlay] = useState('');
  const [multi, setMulti] = useState<{ event: string; result: ExploreRes }[] | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const overlayEvents = overlay.split(',').map((s) => s.trim()).filter(Boolean);
  const runOverlay = () => post<{ series: { event: string; result: ExploreRes }[] }>('/analytics/explore', { events: [event, ...overlayEvents], from: r.from, sum_field: sumField || null, granularity: gran, filter }).then((d) => { setMulti(d.series); setErr(null); }, setErr);
  const run = () => post<ExploreRes>('/analytics/explore', { event, from: r.from, group_by: groupBy || null, sum_field: sumField || null, granularity: gran, filter }).then((d) => { setRes(d); setErr(null); }, setErr);
  const buckets = Array.from(new Set((res?.series ?? []).map((s) => s.bucket))).sort();
  const top = (res?.groups ?? []).slice(0, 6).map((g) => g.group);
  const max = Math.max(1, ...buckets.map((b) => (res?.series ?? []).filter((s) => s.bucket === b).reduce((a, s) => a + s.count, 0)));
  const colors = ['#1a5fb4', '#1e8e3e', '#b26a00', '#a51d2d', '#613583', '#5e5c64'];
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'end' }}>
        {r.picker}
        <Text label={tr('Event')} value={event} onChange={setEvent} mono />
        <Field label={tr('Group by')}><select value={groupBy} onChange={(e) => setGroupBy(e.target.value)}><option value="">none</option><option value="country">country</option><option value="platform">platform</option><option value="app_version">{tr('app version')}</option><option value="experiment:">{tr('experiment: …')}</option><option value="meta:">{tr('metadata: …')}</option></select></Field>
        {(groupBy.startsWith('experiment:') || groupBy.startsWith('meta:')) && <Text label={groupBy.startsWith('meta:') ? 'Field' : 'Experiment key'} value={groupBy.split(':')[1] ?? ''} onChange={(v) => setGroupBy(groupBy.split(':')[0] + ':' + v)} mono />}
        <Text label={tr('Sum metadata field')} value={sumField} onChange={setSumField} mono />
        <Field label={tr('Granularity')}><select value={gran} onChange={(e) => setGran(e.target.value as typeof gran)}><option value="day">day</option><option value="hour">hour</option></select></Field>
        <Text label={tr('Overlay events')} value={overlay} onChange={setOverlay} mono hint={tr('comma-separated, up to 7 more')} />
        <button className="btn" onClick={() => { setMulti(null); run(); }} disabled={!event}>{tr('Run')}</button>
        <button className="btn secondary" onClick={() => { setRes(null); runOverlay(); }} disabled={!event || overlayEvents.length === 0}>{tr('Overlay')}</button>
      </div>
      <ErrorBox error={err} />
      {multi && <OverlayChart series={multi} />}
      {res && <>
        <div className="stats">
          <Stat label={tr('Events')} value={fmtNum(res.total.count)} />
          <Stat label="Σ value" value={money(res.total.sum)} />
          <Stat label={`Σ ${sumField || 'amount'}`} value={money(res.total.field_sum)} />
          <Stat label={tr('Groups')} value={fmtNum(res.groups.length)} />
        </div>
        <div className="card">
          <svg viewBox={`0 0 600 136`} style={{ width: '100%', height: 150 }}>
            {buckets.map((b, i) => {
              const bw = 600 / buckets.length; let y = 120;
              const parts = (res.series.filter((s) => s.bucket === b));
              return <g key={b}>{parts.map((s) => { const h = (s.count / max) * 120; y -= h; const ci = top.indexOf(s.group); return <rect key={s.group} x={i * bw + 1} y={y} width={Math.max(1, bw - 2)} height={h} fill={ci >= 0 ? colors[ci] : '#bbb'}><title>{`${b.slice(0, gran === 'hour' ? 13 : 10)} ${s.group || ''}: ${fmtNum(s.count)}`}</title></rect>; })}</g>;
            })}
          </svg>
          {top.length > 1 && <div className="row" style={{ gap: 12 }}>{top.map((g, i) => <span key={g} className="hint"><span style={{ display: 'inline-block', width: 10, height: 10, background: colors[i], borderRadius: 2, marginRight: 4 }} />{g || '(empty)'}</span>)}</div>}
        </div>
        <Table rows={res.groups} rowKey={(g) => g.group} cols={[
          { h: groupBy ? 'Group' : '', c: (g) => <code>{g.group || (groupBy ? '(empty)' : 'all')}</code> }, { h: 'Events', c: (g) => fmtNum(g.count) }, { h: 'Players', c: (g) => fmtNum(g.identities) },
          { h: 'Σ value', c: (g) => money(g.sum) }, { h: `Σ ${sumField || 'amount'}`, c: (g) => money(g.field_sum) }, { h: 'Share', c: (g) => pct(res.total.count ? g.count / res.total.count : 0) },
        ]} />
      </>}
    </div>
  );
}

function OverlayChart({ series }: { series: { event: string; result: ExploreRes }[] }) {
  const colors = ['#1a5fb4', '#1e8e3e', '#b26a00', '#a51d2d', '#613583', '#5e5c64', '#0e7490', '#9d174d'];
  const buckets = Array.from(new Set(series.flatMap((s) => s.result.series.map((p) => p.bucket)))).sort();
  const perEvent = series.map((s) => buckets.map((b) => s.result.series.filter((p) => p.bucket === b).reduce((a, p) => a + p.count, 0)));
  const max = Math.max(1, ...perEvent.flat());
  const x = (i: number) => (buckets.length > 1 ? (i / (buckets.length - 1)) * 580 + 10 : 300);
  const y = (v: number) => 126 - (v / max) * 110;
  return (
    <div className="card stack">
      <svg viewBox="0 0 600 136" style={{ width: '100%', height: 170 }}>
        {perEvent.map((vals, si) => (
          <polyline key={series[si].event} fill="none" stroke={colors[si % colors.length]} strokeWidth={2} points={vals.map((v, i) => `${x(i)},${y(v)}`).join(' ')} />
        ))}
      </svg>
      <div className="row" style={{ gap: 12, flexWrap: 'wrap' }}>
        {series.map((s, i) => <span key={s.event} className="hint"><span style={{ display: 'inline-block', width: 10, height: 10, background: colors[i % colors.length], marginRight: 4 }} /><code>{s.event}</code> · {fmtNum(s.result.total.count)} events · {fmtNum(s.result.groups.reduce((a, g) => a + g.identities, 0))} players</span>)}
      </div>
      <div className="hint">{buckets[0]} → {buckets[buckets.length - 1]}</div>
    </div>
  );
}

type MapRes = { total_players: number; countries: { country: string; players: number; events: number; share: number }[] };

export function PlayerMap({ filter }: { filter: Filter }) {
  const r = useRangeDays();
  const q = useLoad(() => get<MapRes>('/analytics/map' + qs({ from: r.from, ...filterQs(filter) })), [r.from, JSON.stringify(filter)]);
  const max = Math.max(1, ...(q.data?.countries ?? []).map((c) => c.players));
  const flag = (cc: string) => (/^[A-Z]{2}$/.test(cc) ? String.fromCodePoint(...[...cc].map((c) => 0x1f1e6 + c.charCodeAt(0) - 65)) : '🌐');
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'center' }}>{r.picker}<span className="hint">{fmtNum(q.data?.total_players ?? 0)} players with events in range</span></div>
      <ErrorBox error={q.error} />
      <div className="card map-grid">
        {(q.data?.countries ?? []).slice(0, 60).map((c) => (
          <div key={c.country} className="map-cell" style={{ background: `rgba(26,95,180,${0.08 + 0.8 * (c.players / max)})`, color: c.players / max > 0.5 ? '#fff' : undefined }} title={`${c.country}: ${fmtNum(c.players)} players · ${fmtNum(c.events)} events`}>
            <div className="map-flag">{flag(c.country)}</div><div className="map-cc">{c.country}</div><div className="map-n">{fmtNum(c.players)}</div><div className="map-pct">{pct(c.share)}</div>
          </div>
        ))}
        {q.data && q.data.countries.length === 0 && <div className="hint">{tr("No events in range. Country comes from the identity's default property `country` (set by the SDK or GeoIP).")}</div>}
      </div>
      <Table rows={q.data?.countries} rowKey={(c) => c.country} cols={[
        { h: 'Country', c: (c) => <>{flag(c.country)} <code>{c.country}</code></> }, { h: 'Players', c: (c) => fmtNum(c.players) }, { h: 'Events', c: (c) => fmtNum(c.events) }, { h: 'Share', c: (c) => pct(c.share) },
        { h: '', c: (c) => <div style={{ background: 'var(--border)', height: 8, borderRadius: 4, width: 160 }}><div style={{ width: `${(c.players / max) * 100}%`, background: '#1a5fb4', height: '100%', borderRadius: 4 }} /></div> },
      ]} />
    </div>
  );
}

type PlaytimeRes = { totals: { sessions: number; total_secs: number; avg_session_secs: number; sessions_per_player_day: number; playtime_per_player_day_secs: number }; series: { day: string; sessions: number; players: number; total_secs: number; avg_secs: number; median_secs: number }[]; histogram: { bucket: string; sessions: number }[] };

export function Playtime({ filter }: { filter: Filter }) {
  const r = useRangeDays();
  const q = useLoad(() => get<PlaytimeRes>('/analytics/playtime' + qs({ from: r.from, ...filterQs(filter) })), [r.from, JSON.stringify(filter)]);
  const t = q.data?.totals;
  const hmax = Math.max(1, ...(q.data?.histogram ?? []).map((h) => h.sessions));
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'center' }}>{r.picker}<span className="hint">{tr('from play sessions (platform / app version filters apply)')}</span></div>
      <ErrorBox error={q.error} />
      <div className="stats">
        <Stat label={tr('Sessions')} value={fmtNum(t?.sessions ?? 0)} sub={`${(t?.sessions_per_player_day ?? 0).toFixed(2)} per player-day`} />
        <Stat label={tr('Avg session')} value={mins(t?.avg_session_secs ?? 0)} />
        <Stat label={tr('Playtime / player-day')} value={mins(t?.playtime_per_player_day_secs ?? 0)} />
        <Stat label={tr('Total playtime')} value={`${Math.round((t?.total_secs ?? 0) / 3600)} h`} />
      </div>
      <div className="row">
        <div className="card" style={{ flex: 2 }}>
          <strong>{tr('Sessions per day · avg / median length')}</strong>
          <Table rows={q.data?.series} rowKey={(d) => d.day} cols={[
            { h: 'Day', c: (d) => <code>{d.day}</code> }, { h: 'Sessions', c: (d) => fmtNum(d.sessions) }, { h: 'Players', c: (d) => fmtNum(d.players) },
            { h: 'Avg', c: (d) => mins(d.avg_secs) }, { h: 'Median', c: (d) => mins(d.median_secs) }, { h: 'Total', c: (d) => `${(d.total_secs / 3600).toFixed(1)} h` },
          ]} />
        </div>
        <div className="card" style={{ flex: 1 }}>
          <strong>{tr('Session length')}</strong>
          {(q.data?.histogram ?? []).map((h) => <div key={h.bucket} className="row" style={{ alignItems: 'center', margin: '6px 0' }}><span className="mono" style={{ width: 56 }}>{h.bucket}</span><div style={{ flex: 1, background: 'var(--border)', height: 14, borderRadius: 4 }}><div style={{ width: `${(h.sessions / hmax) * 100}%`, background: '#1e8e3e', height: '100%', borderRadius: 4 }} /></div><span style={{ width: 60, textAlign: 'right' }}>{fmtNum(h.sessions)}</span></div>)}
        </div>
      </div>
    </div>
  );
}

type LtvRes = { days: number[]; totals: { installs: number; spend: number; cpi: number; at: Record<string, { revenue: number; ltv: number; roas: number }> }; cohorts: { cohort: string; installs: number; reported_installs: number; spend: number; cpi: number; at: Record<string, { revenue: number; ltv: number; roas: number } | null> }[] };
type SpendRow = { day: string; platform: string; country: string; campaign: string; spend: number; installs: number; currency: string };

export function Ltv({ filter }: { filter: Filter }) {
  const r = useRangeDays(30);
  const [days, setDays] = useState('1,3,7,14,30');
  const q = useLoad(() => get<LtvRes>('/analytics/ltv' + qs({ from: r.from, days, ...filterQs(filter) })), [r.from, days, JSON.stringify(filter)]);
  const spend = useLoad(() => get<{ items: SpendRow[] }>('/analytics/ad-spend' + qs({ from: r.from })), [r.from]);
  const [csv, setCsv] = useState('');
  const [row, setRow] = useState<SpendRow>({ day: new Date().toISOString().slice(0, 10), platform: '', country: '', campaign: '', spend: 0, installs: 0, currency: 'USD' });
  const [err, setErr] = useState<Error | null>(null);
  const importCsv = () => post('/analytics/ad-spend', { csv }).then(() => { setCsv(''); q.reload(); spend.reload(); }, setErr);
  const addRow = () => post('/analytics/ad-spend', { rows: [row] }).then(() => { q.reload(); spend.reload(); }, setErr);
  const t = q.data?.totals;
  const cell = (v: { revenue: number; ltv: number; roas: number } | null | undefined, showRoas: boolean) => v == null ? <span className="hint">—</span> : <span title={`revenue ${money(v.revenue)}`}>{money(v.ltv)}{showRoas && <span className="hint"> · {pct(v.roas)}</span>}</span>;
  return (
    <div className="stack">
      <div className="row" style={{ alignItems: 'end' }}>{r.picker}<Text label={tr('Day offsets')} value={days} onChange={setDays} mono hint={tr('LTV / RoAS at D-n, 0..180')} /></div>
      <ErrorBox error={q.error ?? spend.error ?? err} />
      <div className="stats">
        <Stat label={tr('Installs')} value={fmtNum(t?.installs ?? 0)} sub="new identities in range" />
        <Stat label={tr('Ad spend')} value={money(t?.spend ?? 0)} sub={`CPI ${money(t?.cpi ?? 0)}`} />
        {(q.data?.days ?? []).slice(0, 2).map((d) => <Stat key={d} label={`LTV D${d}`} value={money(t?.at[`d${d}`]?.ltv)} sub={`RoAS ${pct(t?.at[`d${d}`]?.roas)}`} />)}
      </div>
      <div className="card">
        <strong>{tr('Install cohorts')}</strong> <span className="hint">{tr('LTV = cumulative revenue per install · RoAS = revenue / spend')}</span>
        <Table rows={q.data?.cohorts} rowKey={(c) => c.cohort} empty={tr('No installs or spend in this range')} cols={[
          { h: 'Cohort', c: (c) => <code>{c.cohort}</code> }, { h: 'Installs', c: (c) => <>{fmtNum(c.installs)}{c.reported_installs > 0 && <span className="hint"> / {fmtNum(c.reported_installs)} reported</span>}</> },
          { h: 'Spend', c: (c) => money(c.spend) }, { h: 'CPI', c: (c) => money(c.cpi) },
          ...(q.data?.days ?? []).map((d) => ({ h: `D${d}`, c: (c: LtvRes['cohorts'][number]) => cell(c.at[`d${d}`], c.spend > 0) })),
        ]} />
      </div>
      <div className="row" style={{ alignItems: 'stretch' }}>
        <div className="card stack" style={{ flex: 1 }}>
          <strong>{tr('Import ad spend (CSV)')}</strong>
          <span className="hint">{tr('day,platform,country,campaign,spend,installs[,currency] — platform / country may be empty, rows upsert by (day, platform, country, campaign)')}</span>
          <textarea rows={5} className="mono" value={csv} onChange={(e) => setCsv(e.target.value)} placeholder={'2026-10-01,ios,DE,launch-eu,1250.00,410\n2026-10-01,android,,launch-global,980.50,620'} />
          <div><button className="btn" disabled={!csv.trim()} onClick={importCsv}>{tr('Import')}</button></div>
        </div>
        <div className="card stack" style={{ flex: 1 }}>
          <strong>{tr('Add a row')}</strong>
          <div className="row" style={{ alignItems: 'end' }}>
            <Field label={tr('Day')}><input type="date" value={row.day} onChange={(e) => setRow({ ...row, day: e.target.value })} /></Field>
            <Text label={tr('Platform')} value={row.platform} onChange={(platform) => setRow({ ...row, platform })} mono />
            <Text label={tr('Country')} value={row.country} onChange={(country) => setRow({ ...row, country: country.toUpperCase() })} mono />
          </div>
          <div className="row" style={{ alignItems: 'end' }}>
            <Text label={tr('Campaign')} value={row.campaign} onChange={(campaign) => setRow({ ...row, campaign })} />
            <Num label={tr('Spend')} value={row.spend} onChange={(v) => setRow({ ...row, spend: v ?? 0 })} min={0} />
            <Num label={tr('Installs')} value={row.installs} onChange={(v) => setRow({ ...row, installs: v ?? 0 })} min={0} />
            <button className="btn" onClick={addRow}>{tr('Save')}</button>
          </div>
        </div>
      </div>
      <Table rows={spend.data?.items} rowKey={(s) => `${s.day}${s.platform}${s.country}${s.campaign}`} empty={tr('No ad spend recorded in range')} cols={[
        { h: 'Day', c: (s) => <code>{s.day}</code> }, { h: 'Platform', c: (s) => s.platform || <span className="hint">any</span> }, { h: 'Country', c: (s) => s.country || <span className="hint">any</span> },
        { h: 'Campaign', c: (s) => s.campaign }, { h: 'Spend', c: (s) => `${money(s.spend)} ${s.currency}` }, { h: 'Installs', c: (s) => fmtNum(s.installs) },
      ]} />
    </div>
  );
}

type ErrorsRes = { since: string; rejected_events: { count: number; by_reason: Record<string, number>; recent: { at: string; name: string; reason: string }[] }; failures: { source: string; count: number }[] };

export function Errors() {
  const q = useLoad(() => get<ErrorsRes>('/analytics/errors'));
  const d = q.data;
  return (
    <div className="stack">
      <ErrorBox error={q.error} />
      <div className="stats">
        <Stat label={tr('Rejected events')} value={fmtNum(d?.rejected_events.count ?? 0)} sub={<Link to="/taxonomy">{tr('taxonomy debugger')}</Link>} />
        {d?.failures.map((f) => <Stat key={f.source} label={f.source.replace('_', ' ')} value={<span style={{ color: f.count > 0 ? 'var(--bad)' : undefined }}>{fmtNum(f.count)}</span>} sub="failed in the last 24 h" />)}
      </div>
      <div className="row" style={{ alignItems: 'stretch' }}>
        <div className="card" style={{ flex: 1 }}>
          <strong>{tr('Rejections by reason')}</strong>
          {Object.entries(d?.rejected_events.by_reason ?? {}).sort((a, b) => b[1] - a[1]).map(([r, n]) => <div key={r} className="row" style={{ justifyContent: 'space-between' }}><span>{r}</span><strong>{fmtNum(n)}</strong></div>)}
          {d && d.rejected_events.count === 0 && <div className="hint">{tr('No rejected events. Validation mode is configured per event definition in Taxonomy.')}</div>}
        </div>
        <div className="card" style={{ flex: 2 }}>
          <strong>{tr('Recent rejections')}</strong>
          <Table rows={d?.rejected_events.recent} rowKey={(r) => `${r.at}${r.name}${r.reason}`} cols={[{ h: 'When', c: (r) => fmtDate(r.at) }, { h: 'Event', c: (r) => <code>{r.name}</code> }, { h: 'Reason', c: (r) => r.reason }]} />
        </div>
      </div>
    </div>
  );
}
