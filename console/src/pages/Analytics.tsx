import { useState } from 'react';
import { get, qs, useLoad } from '../api';
import { ErrorBox, Page, Table, fmtNum } from '../ui';
import { t } from '../i18n';

type Names = { backend: string; from: string; to: string; names: { name: string; count: number; identities: number }[] };
type Bucket = { bucket: string; count: number; identities: number };
type Series = { name: string; granularity: string; buckets: Bucket[] };

const RANGES: [string, number][] = [['24h', 1], ['7d', 7], ['30d', 30], ['90d', 90]];
const COLORS = ['var(--fg)', '#1a5fb4', '#1e8e3e', '#b26a00'];

function useSeries(names: string[], from: string, g: string) {
  return useLoad(() => Promise.all(names.map((n) => get<Series>('/analytics/events' + qs({ name: n, from, granularity: g })))), [names.join('|'), from, g]);
}

function Chart({ series, mode }: { series: Series[]; mode: 'count' | 'identities' }) {
  const [hover, setHover] = useState<number | null>(null);
  const base = series[0]?.buckets ?? [];
  if (!base.length) return <div className="muted empty">{t('No events in this range')}</div>;
  const n = base.length;
  const val = (s: Series, i: number) => s.buckets[i]?.[mode] ?? 0;
  const max = Math.max(1, ...series.flatMap((s) => s.buckets.map((b) => b[mode])));
  const W = 800, H = 240, L = 40, B = 26, T = 8;
  const x = (i: number) => L + (i / Math.max(n - 1, 1)) * (W - L - 8);
  const y = (v: number) => T + (1 - v / max) * (H - T - B);
  const g = series[0].granularity;
  const label = (i: number) => new Date(base[i].bucket).toLocaleString(undefined, g === 'hour' ? { hour: '2-digit', minute: '2-digit' } : { month: 'short', day: 'numeric' });
  return (
    <div style={{ position: 'relative' }}>
      <svg viewBox={`0 0 ${W} ${H}`} style={{ width: '100%', height: 280, display: 'block' }}
        onMouseMove={(e) => { const r = e.currentTarget.getBoundingClientRect(); const px = ((e.clientX - r.left) / r.width) * W; const i = Math.round(((px - L) / (W - L - 8)) * (n - 1)); setHover(Math.max(0, Math.min(n - 1, i))); }}
        onMouseLeave={() => setHover(null)}>
        {[0, 0.25, 0.5, 0.75, 1].map((f) => <g key={f}><line x1={L} x2={W} y1={y(max * f)} y2={y(max * f)} stroke="var(--line)" /><text x={L - 6} y={y(max * f) + 4} fontSize="10" textAnchor="end" fill="var(--muted)">{fmtNum(Math.round(max * f))}</text></g>)}
        {base.map((_, i) => (i % Math.ceil(n / 8) === 0) && <text key={i} x={x(i)} y={H - 8} fontSize="10" textAnchor="middle" fill="var(--muted)">{label(i)}</text>)}
        {series.map((s, si) => {
          const c = COLORS[si % COLORS.length];
          const pts = base.map((_, i) => `${x(i)},${y(val(s, i))}`).join(' ');
          return (
            <g key={s.name}>
              <polygon points={`${x(0)},${y(0)} ${pts} ${x(n - 1)},${y(0)}`} fill={c} opacity={si === 0 ? 0.08 : 0.04} />
              <polyline points={pts} fill="none" stroke={c} strokeWidth="2" strokeLinejoin="round" />
              {hover !== null && <circle cx={x(hover)} cy={y(val(s, hover))} r="4" fill={c} />}
            </g>
          );
        })}
        {hover !== null && <line x1={x(hover)} x2={x(hover)} y1={T} y2={H - B} stroke="var(--muted)" strokeDasharray="3 3" />}
      </svg>
      {hover !== null && (
        <div className="tooltip" style={{ left: `${(x(hover) / W) * 100}%` }}>
          <div className="hint">{new Date(base[hover].bucket).toLocaleString()}</div>
          {series.map((s, si) => <div key={s.name}><span style={{ color: COLORS[si % COLORS.length] }}>●</span> {s.name}: <strong>{fmtNum(val(s, hover))}</strong></div>)}
        </div>
      )}
    </div>
  );
}

export default function Analytics() {
  const [days, setDays] = useState(7);
  const [names, setNames] = useState<string[]>(['$authenticate']);
  const [mode, setMode] = useState<'count' | 'identities'>('count');
  const from = new Date(Date.now() - days * 86400e3).toISOString();
  const g = days <= 1 ? 'hour' : 'day';
  const list = useLoad(() => get<Names>('/analytics/events/names' + qs({ from, limit: 100 })), [days]);
  const series = useSeries(names, from, g);
  const toggle = (n: string) => setNames(names.includes(n) ? (names.length > 1 ? names.filter((x) => x !== n) : names) : names.length >= 4 ? [...names.slice(1), n] : [...names, n]);
  const totals = (series.data ?? []).map((s) => s.buckets.reduce((a, b) => a + b[mode], 0));
  const maxCount = Math.max(1, ...(list.data?.names.map((x) => x.count) ?? [1]));
  return (
    <Page title={t('Analytics')} actions={
      <>
        <div className="tabs" style={{ border: 0, margin: 0 }}>{(['count', 'identities'] as const).map((m) => <button key={m} className={mode === m ? 'active' : ''} onClick={() => setMode(m)}>{m === 'count' ? 'events' : t('unique identities')}</button>)}</div>
        <div className="tabs" style={{ border: 0, margin: 0 }}>{RANGES.map(([l, d]) => <button key={l} className={days === d ? 'active' : ''} onClick={() => setDays(d)}>{l}</button>)}</div>
      </>
    }>
      <ErrorBox error={list.error ?? series.error} />
      <div className="card">
        <div className="legend">
          {names.map((n, i) => <button key={n} className="chip on" style={{ background: COLORS[i % COLORS.length], borderColor: COLORS[i % COLORS.length], color: i === 0 ? 'var(--bg)' : '#fff' }} onClick={() => toggle(n)} title={t('remove from chart')}>{n} · {fmtNum(totals[i] ?? 0)}</button>)}
          <span className="hint">per {g} {t('· backend')} {list.data?.backend ?? '…'} {t('· click rows below to compare up to 4 events')}</span>
        </div>
        {series.data && <Chart series={series.data} mode={mode} />}
      </div>
      <Table
        rows={list.data?.names}
        rowKey={(n) => n.name}
        onRow={(n) => toggle(n.name)}
        empty={t('No events in this range')}
        cols={[
          { h: t('Event'), c: (n) => <code style={{ fontWeight: names.includes(n.name) ? 700 : 400 }}>{names.includes(n.name) && <span style={{ color: COLORS[names.indexOf(n.name) % COLORS.length] }}>● </span>}{n.name}</code> },
          { h: t('Events'), c: (n) => fmtNum(n.count) },
          { h: t('Identities'), c: (n) => fmtNum(n.identities) },
          { h: t('Per identity'), c: (n) => (n.count / Math.max(n.identities, 1)).toFixed(2) },
          { h: '', c: (n) => <div className="bar"><i style={{ width: `${(n.count / maxCount) * 100}%` }} /></div> },
        ]}
      />
      <p className="hint">{t('Names starting with') + ' '}<code>$</code>{' ' + t('are reserved server events ($authenticate, $grant, $expose, …).')}</p>
    </Page>
  );
}
