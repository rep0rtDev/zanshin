import { useEffect, useState } from 'react';
import { get, useLoad } from '../api';
import { Badge, ErrorBox, Stat, Table, fmtDate } from '../ui';
import { t } from '../i18n';

type Comp = { ok: boolean; latency_ms?: number; error?: string | null };
type Job = { name: string; interval_secs: number; stale: boolean; last_started_at?: string | null; last_finished_at?: string | null; last_seen_at?: string | null; last_ms: number; runs: number; skipped: number; last_error?: string | null };
type Integ = { key: string; name: string; kind: string; enabled: boolean; last_status?: string | null; last_at?: string | null; last_error?: string | null; sent: number; failed: number };
type Sink = { key: string; name: string; kind: string; enabled: boolean; last_run_at?: string | null; next_run_at?: string | null; last_status?: string | null; last_error?: string | null; last_rows?: number | null };
type Report = { key: string; name: string; enabled: boolean; cron: string; last_run_at?: string | null; last_status: string };
type Alert = { key: string; name: string; enabled: boolean; event: string; last_checked_at?: string | null; last_fired_at?: string | null; last_value?: number | null; firing: boolean };
type Overview = {
  status: 'ok' | 'warn' | 'fail'; issues: string[]; checked_at: string;
  process: { version: string; project: string; node_id: string; started_at: string; uptime_seconds: number; draining: boolean; production: boolean };
  dependencies: { ok: boolean; postgres: Comp; redis: Comp; clickhouse?: Comp; pool: { size: number; idle: number; max: number } };
  ingest: { sink: string; queue_depth: number; queue_capacity: number; events_5m: number; events_1h: number; identities_1h: number; dropped_total: number; rate_limited_total: number };
  streams: { websocket: number; server_sdk: number };
  jobs: Job[]; integrations: Integ[];
  webhooks: { configured: number; enabled: number; deliveries: { pending: number; retrying: number; dead_24h: number; sent_24h: number; oldest_pending_secs: number } };
  exports: Sink[]; reports: Report[]; alerts: Alert[];
};

const dur = (s: number) => s < 90 ? `${s}s` : s < 5400 ? `${Math.round(s / 60)}m` : s < 172800 ? `${Math.round(s / 3600)}h` : `${Math.round(s / 86400)}d`;
const ago = (s?: string | null) => (s ? dur(Math.max(0, Math.round((Date.now() - new Date(s).getTime()) / 1000))) + ' ago' : '—');
const okBadge = (ok: boolean, label?: string) => <Badge tone={ok ? 'ok' : 'bad'}>{label ?? (ok ? 'ok' : 'down')}</Badge>;
const statusTone = (s?: string | null) => (s === 'ok' || s === 'sent' || s === 'succeeded' ? 'ok' : s === 'failed' || s?.startsWith('failed') ? 'bad' : s === 'skipped' ? 'muted' : 'info');

export default function Health() {
  const [tick, setTick] = useState(0);
  const [auto, setAuto] = useState(true);
  useEffect(() => {
    if (!auto) return;
    const t = setInterval(() => setTick((n) => n + 1), 15000);
    return () => clearInterval(t);
  }, [auto]);
  const q = useLoad(() => get<Overview>('/health'), [tick]);
  const h = q.data;
  const tone = h?.status === 'ok' ? 'ok' : h?.status === 'warn' ? 'warn' : 'bad';
  const deps: { name: string; c: Comp }[] = h ? [{ name: 'PostgreSQL', c: h.dependencies.postgres }, { name: 'Redis', c: h.dependencies.redis }, ...(h.dependencies.clickhouse ? [{ name: 'ClickHouse', c: h.dependencies.clickhouse }] : [])] : [];
  return (
    <div className="page">
      <div className="page-head">
        <h1>{t('Health')} {h && <Badge tone={tone}>{h.status}</Badge>}</h1>
        <p className="blurb">{t('Is this deployment doing its background work? Dependencies, ingest queue, housekeeping jobs and every delivery channel in one place. Refreshes every 15 s.')}</p>
        <div className="actions">
          <label className="check"><input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} /> auto-refresh</label>
          <button onClick={() => setTick((n) => n + 1)}>{t('Refresh')}</button>
        </div>
      </div>
      <ErrorBox error={q.error} />
      {h && (
        <>
          {h.issues.length > 0 && (
            <div className={'card issues ' + tone}>
              <strong>{h.issues.length === 1 ? '1 issue' : `${h.issues.length} issues`}</strong>
              <ul>{h.issues.map((i, n) => <li key={n}>{i}</li>)}</ul>
            </div>
          )}
          <div className="grid c4 stats">
            <Stat label="uptime" value={dur(h.process.uptime_seconds)} sub={`v${h.process.version} · ${h.process.node_id.slice(0, 8)}${h.process.draining ? ' · draining' : ''}`} />
            <Stat label={t('ingest queue')} value={`${h.ingest.queue_depth} / ${h.ingest.queue_capacity}`} sub={`${h.ingest.sink} · ${h.ingest.dropped_total} dropped`} />
            <Stat label={t('events (5 m / 1 h)')} value={`${h.ingest.events_5m} / ${h.ingest.events_1h}`} sub={`${h.ingest.identities_1h} players in the last hour`} />
            <Stat label={t('live streams')} value={h.streams.websocket + h.streams.server_sdk} sub={`${h.streams.websocket} console · ${h.streams.server_sdk} server SDK`} />
          </div>
          <div className="grid c2">
            <section className="card">
              <h2>{t('Dependencies')}</h2>
              <Table cols={[
                { h: t('Service'), c: (d) => <strong>{d.name}</strong> },
                { h: t('Status'), c: (d) => okBadge(d.c.ok) },
                { h: t('Latency'), c: (d) => d.c.latency_ms != null ? `${d.c.latency_ms} ms` : '—' },
                { h: '', c: (d) => <span className="muted">{d.c.error ?? ''}</span> },
              ]} rows={deps} rowKey={(d) => d.name} />
              <div className="hint" style={{ marginTop: 8 }}>Postgres pool: {h.dependencies.pool.size} open, {h.dependencies.pool.idle} idle, max {h.dependencies.pool.max}.</div>
            </section>
            <section className="card">
              <h2>{t('Webhooks') + ' '}<span className="count">{h.webhooks.enabled}/{h.webhooks.configured} enabled</span></h2>
              <div className="grid c4 stats">
                <Stat label="pending" value={h.webhooks.deliveries.pending} />
                <Stat label="retrying" value={h.webhooks.deliveries.retrying} />
                <Stat label={t('sent 24 h')} value={h.webhooks.deliveries.sent_24h} />
                <Stat label={t('dead 24 h')} value={h.webhooks.deliveries.dead_24h} />
              </div>
              <div className="hint">{h.webhooks.deliveries.oldest_pending_secs > 0 ? `Oldest undelivered: ${dur(h.webhooks.deliveries.oldest_pending_secs)}.` : 'No backlog.'}</div>
            </section>
          </div>
          <section className="card">
            <h2>{t('Housekeeping jobs') + ' '}<span className="count">{t('this replica')}</span></h2>
            <Table cols={[
              { h: t('Job'), c: (j) => <strong>{j.name}</strong> },
              { h: t('State'), c: (j) => j.last_error ? <Badge tone="bad">failed</Badge> : j.stale ? <Badge tone="warn">stale</Badge> : <Badge tone="ok">ok</Badge> },
              { h: t('Last run'), c: (j) => ago(j.last_finished_at) },
              { h: t('Took'), c: (j) => `${j.last_ms} ms` },
              { h: t('Every'), c: (j) => dur(j.interval_secs) },
              { h: t('Runs / skipped'), c: (j) => `${j.runs} / ${j.skipped}` },
              { h: '', c: (j) => <span className="muted">{j.last_error ?? ''}</span> },
            ]} rows={h.jobs} rowKey={(j) => j.name} empty={t('Housekeeping has not reported yet — first jobs run within a minute of start.')} />
          </section>
          <section className="card">
            <h2>{t('Integrations')}</h2>
            <Table cols={[
              { h: t('Integration'), c: (i) => <><strong>{i.name || i.key}</strong> <span className="muted">{i.kind}</span></> },
              { h: t('Enabled'), c: (i) => okBadge(i.enabled, i.enabled ? 'on' : 'off') },
              { h: t('Last delivery'), c: (i) => i.last_status ? <Badge tone={statusTone(i.last_status)}>{i.last_status}</Badge> : <span className="muted">never</span> },
              { h: t('When'), c: (i) => ago(i.last_at) },
              { h: t('Sent / failed'), c: (i) => `${i.sent} / ${i.failed}` },
              { h: '', c: (i) => <span className="muted">{i.last_error ?? ''}</span> },
            ]} rows={h.integrations} rowKey={(i) => i.key} empty={t('No integrations configured.')} />
          </section>
          <div className="grid c2">
            <section className="card">
              <h2>{t('Export sinks')}</h2>
              <Table cols={[
                { h: t('Sink'), c: (s) => <><strong>{s.name || s.key}</strong> <span className="muted">{s.kind}</span></> },
                { h: t('Last run'), c: (s) => s.last_status ? <Badge tone={statusTone(s.last_status)}>{s.last_status}</Badge> : <span className="muted">never</span> },
                { h: t('When'), c: (s) => ago(s.last_run_at) },
                { h: t('Next'), c: (s) => s.enabled ? fmtDate(s.next_run_at) : <span className="muted">disabled</span> },
              ]} rows={h.exports} rowKey={(s) => s.key} empty={t('No export sinks.')} />
            </section>
            <section className="card">
              <h2>{t('Scheduled reports')}</h2>
              <Table cols={[
                { h: t('Report'), c: (r) => <strong>{r.name || r.key}</strong> },
                { h: t('Cron'), c: (r) => <code>{r.cron}</code> },
                { h: t('Last run'), c: (r) => r.last_status ? <Badge tone={statusTone(r.last_status)}>{r.last_status}</Badge> : <span className="muted">never</span> },
                { h: t('When'), c: (r) => ago(r.last_run_at) },
              ]} rows={h.reports} rowKey={(r) => r.key} empty={t('No scheduled reports.')} />
            </section>
          </div>
          <section className="card">
            <h2>{t('Metric alerts')}</h2>
            <Table cols={[
              { h: t('Alert'), c: (a) => <><strong>{a.name || a.key}</strong> <span className="muted">{a.event}</span></> },
              { h: t('State'), c: (a) => !a.enabled ? <Badge tone="muted">off</Badge> : a.firing ? <Badge tone="bad">firing</Badge> : <Badge tone="ok">quiet</Badge> },
              { h: t('Last value'), c: (a) => a.last_value ?? '—' },
              { h: t('Checked'), c: (a) => ago(a.last_checked_at) },
              { h: t('Last fired'), c: (a) => ago(a.last_fired_at) },
            ]} rows={h.alerts} rowKey={(a) => a.key} empty={t('No metric alerts.')} />
          </section>
          <div className="hint">Checked {fmtDate(h.checked_at)} · {h.process.production ? 'production' : 'development'} mode · project {h.process.project}</div>
        </>
      )}
    </div>
  );
}
