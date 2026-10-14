import { Link } from 'react-router-dom';
import { get, useLoad } from '../api';
import { ErrorBox, Page, Sparkline, Stat, fmtNum } from '../ui';

type Stats = { identities: number; uptime_seconds: number; clickhouse_enabled: boolean };
type Series = { buckets: { bucket: string; count: number; identities: number }[] };
const since = new Date(Date.now() - 24 * 3600e3).toISOString();

export default function Dashboard() {
  const stats = useLoad(() => get<Stats>('/stats'));
  const flags = useLoad(() => get<{ enabled: boolean }[]>('/flags'));
  const exps = useLoad(() => get<{ status: string }[]>('/experiments'));
  const auds = useLoad(() => get<unknown[]>('/audiences'));
  const events = useLoad(() => get<{ enabled: boolean }[]>('/live-events'));
  const msgs = useLoad(() => get<{ enabled: boolean }[]>('/messages'));
  const push = useLoad(() => get<{ mode: string; tokens: number; identities: number }>('/push/stats'));
  const auth = useLoad(() => get<Series>(`/analytics/events?name=%24authenticate&granularity=hour&from=${since}`));
  const s = stats.data;
  const up = s ? `${Math.floor(s.uptime_seconds / 3600)}h ${Math.floor((s.uptime_seconds % 3600) / 60)}m` : '';
  return (
    <Page title="Overview">
      <ErrorBox error={stats.error} />
      <div className="grid c4">
        <Stat label="Identities" value={fmtNum(s?.identities ?? 0)} sub={s ? `server up ${up} · analytics on ${s.clickhouse_enabled ? 'ClickHouse' : 'Postgres'}` : undefined} />
        <Stat label="Feature flags" value={<Link to="/flags">{fmtNum(flags.data?.length ?? 0)}</Link>} sub={flags.data ? `${flags.data.filter((f) => f.enabled).length} enabled` : undefined} />
        <Stat label="Experiments" value={<Link to="/experiments">{fmtNum(exps.data?.length ?? 0)}</Link>} sub={exps.data ? `${exps.data.filter((e) => e.status === 'running').length} running` : undefined} />
        <Stat label="Audiences" value={<Link to="/audiences">{fmtNum(auds.data?.length ?? 0)}</Link>} />
        <Stat label="Live events" value={<Link to="/live-events">{fmtNum(events.data?.length ?? 0)}</Link>} sub={events.data ? `${events.data.filter((e) => e.enabled).length} enabled` : undefined} />
        <Stat label="Message campaigns" value={<Link to="/messages">{fmtNum(msgs.data?.length ?? 0)}</Link>} sub={msgs.data ? `${msgs.data.filter((m) => m.enabled).length} enabled` : undefined} />
        <Stat label="Push devices" value={fmtNum(push.data?.tokens ?? 0)} sub={push.data ? `${fmtNum(push.data.identities)} identities · provider ${push.data.mode}` : undefined} />
        <Stat label="Sessions, 24h" value={fmtNum((auth.data?.buckets ?? []).reduce((a, b) => a + b.count, 0))} sub="$authenticate events" />
      </div>
      <div className="card" style={{ marginTop: 16 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between' }}>
          <strong>Authentications per hour, last 24h</strong>
          <Link to="/analytics" className="muted">analytics →</Link>
        </div>
        <Sparkline points={(auth.data?.buckets ?? []).map((b) => b.count)} height={80} />
      </div>
    </Page>
  );
}
