import { useState } from 'react';
import { get, qs, useLoad } from '../api';
import { ErrorBox, Page, Table, fmtNum } from '../ui';

type Names = { backend: string; from: string; to: string; names: { name: string; count: number; identities: number }[] };
type Series = { granularity: string; buckets: { bucket: string; count: number; identities: number }[] };

const RANGES: [string, number][] = [['24h', 1], ['7d', 7], ['30d', 30], ['90d', 90]];

function Chart({ s }: { s: Series }) {
  const b = s.buckets;
  if (!b.length) return <div className="muted empty">No events in this range</div>;
  const max = Math.max(...b.map((x) => x.count), 1);
  const W = 800, H = 220, pad = 36, bw = (W - pad) / b.length;
  const y = (v: number) => H - 24 - (v / max) * (H - 40);
  return (
    <svg viewBox={`0 0 ${W} ${H}`} style={{ width: '100%', height: 260, display: 'block' }}>
      {[0, 0.5, 1].map((f) => <g key={f}><line x1={pad} x2={W} y1={y(max * f)} y2={y(max * f)} stroke="#eee" /><text x={pad - 6} y={y(max * f) + 4} fontSize="10" textAnchor="end" fill="#888">{fmtNum(Math.round(max * f))}</text></g>)}
      {b.map((x, i) => (
        <g key={x.bucket}>
          <rect x={pad + i * bw + 1} width={Math.max(bw - 2, 1)} y={y(x.count)} height={H - 24 - y(x.count)} fill="#111" rx="1">
            <title>{new Date(x.bucket).toLocaleString()}: {fmtNum(x.count)} events, {fmtNum(x.identities)} identities</title>
          </rect>
          <rect x={pad + i * bw + 1} width={Math.max(bw - 2, 1)} y={y(x.identities)} height={H - 24 - y(x.identities)} fill="#8ab4f8" opacity="0.7" rx="1" />
          {(i % Math.ceil(b.length / 8) === 0) && <text x={pad + i * bw + bw / 2} y={H - 8} fontSize="10" textAnchor="middle" fill="#888">{new Date(x.bucket).toLocaleDateString(undefined, s.granularity === 'hour' ? { hour: '2-digit', minute: '2-digit' } : { month: 'short', day: 'numeric' })}</text>}
        </g>
      ))}
    </svg>
  );
}

export default function Analytics() {
  const [days, setDays] = useState(7);
  const [name, setName] = useState('$authenticate');
  const from = new Date(Date.now() - days * 86400e3).toISOString();
  const g = days <= 1 ? 'hour' : 'day';
  const names = useLoad(() => get<Names>('/analytics/events/names' + qs({ from, limit: 100 })), [days]);
  const series = useLoad(() => get<Series>('/analytics/events' + qs({ name, from, granularity: g })), [name, days]);
  const total = series.data?.buckets.reduce((a, b) => a + b.count, 0) ?? 0;
  return (
    <Page title="Analytics" actions={<div className="tabs" style={{ border: 0, margin: 0 }}>{RANGES.map(([l, d]) => <button key={l} className={days === d ? 'active' : ''} onClick={() => setDays(d)}>{l}</button>)}</div>}>
      <ErrorBox error={names.error ?? series.error} />
      <div className="card">
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline' }}>
          <strong><code>{name}</code> · {fmtNum(total)} events per {g}</strong>
          <span className="hint">black = events, blue = unique identities · backend {names.data?.backend ?? '…'}</span>
        </div>
        {series.data && <Chart s={series.data} />}
      </div>
      <Table
        rows={names.data?.names}
        rowKey={(n) => n.name}
        onRow={(n) => setName(n.name)}
        empty="No events in this range"
        cols={[
          { h: 'Event', c: (n) => <code style={{ fontWeight: n.name === name ? 700 : 400 }}>{n.name}</code> },
          { h: 'Count', c: (n) => fmtNum(n.count) },
          { h: 'Identities', c: (n) => fmtNum(n.identities) },
          { h: 'Per identity', c: (n) => (n.count / Math.max(n.identities, 1)).toFixed(2) },
          { h: '', c: (n) => <div className="bar"><i style={{ width: `${(n.count / Math.max(...names.data!.names.map((x) => x.count), 1)) * 100}%` }} /></div> },
        ]}
      />
      <p className="hint">Names starting with <code>$</code> are reserved server events ($authenticate, $grant, $expose, …). Click a row to chart it.</p>
    </Page>
  );
}
