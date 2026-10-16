import { useState } from 'react';
import { Link } from 'react-router-dom';
import { get, useLoad } from '../api';
import { ErrorBox, Page, Sparkline, Stat, fmtNum } from '../ui';
import { t } from '../i18n';

type Stats = { identities: number; uptime_seconds: number; clickhouse_enabled: boolean };
type Series = { buckets: { bucket: string; count: number; identities: number }[] };
type Onboarding = { steps: { id: string; title: string; done: boolean; href: string; hint: string }[]; done: number; total: number };
const DISMISS_KEY = 'zanshin.onboarding.dismissed';
const UPDATE_KEY = 'zanshin.update.dismissed';
type Updates = { enabled: boolean; current: string; latest?: string | null; update_available: boolean; url?: string | null };
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
  const onboarding = useLoad(() => get<Onboarding>('/onboarding'));
  const updates = useLoad(() => get<Updates>('/updates'));
  const [hiddenUpdate, setHiddenUpdate] = useState(() => localStorage.getItem(UPDATE_KEY));
  const [dismissed, setDismissed] = useState(() => localStorage.getItem(DISMISS_KEY) === '1');
  const s = stats.data;
  const up = s ? `${Math.floor(s.uptime_seconds / 3600)}h ${Math.floor((s.uptime_seconds % 3600) / 60)}m` : '';
  return (
    <Page title={t('Overview')}>
      <ErrorBox error={stats.error} />
      {updates.data?.update_available && updates.data.latest && hiddenUpdate !== updates.data.latest && (
        <UpdateBanner
          data={updates.data}
          onDismiss={() => {
            localStorage.setItem(UPDATE_KEY, updates.data!.latest!);
            setHiddenUpdate(updates.data!.latest!);
          }}
        />
      )}
      {onboarding.data && onboarding.data.done < onboarding.data.total && !dismissed && (
        <GettingStarted
          data={onboarding.data}
          onDismiss={() => {
            localStorage.setItem(DISMISS_KEY, '1');
            setDismissed(true);
          }}
        />
      )}
      <div className="grid c4">
        <Stat label={t('Identities')} value={fmtNum(s?.identities ?? 0)} sub={s ? `${t('server up')} ${up} · ${t('analytics on')} ${s.clickhouse_enabled ? 'ClickHouse' : 'Postgres'}` : undefined} />
        <Stat label={t('Feature flags')} value={<Link to="/flags">{fmtNum(flags.data?.length ?? 0)}</Link>} sub={flags.data ? `${flags.data.filter((f) => f.enabled).length} ${t('enabled')}` : undefined} />
        <Stat label={t('Experiments')} value={<Link to="/experiments">{fmtNum(exps.data?.length ?? 0)}</Link>} sub={exps.data ? `${exps.data.filter((e) => e.status === 'running').length} ${t('running')}` : undefined} />
        <Stat label={t('Audiences')} value={<Link to="/audiences">{fmtNum(auds.data?.length ?? 0)}</Link>} />
        <Stat label={t('Live events')} value={<Link to="/live-events">{fmtNum(events.data?.length ?? 0)}</Link>} sub={events.data ? `${events.data.filter((e) => e.enabled).length} ${t('enabled')}` : undefined} />
        <Stat label={t('Message campaigns')} value={<Link to="/messages">{fmtNum(msgs.data?.length ?? 0)}</Link>} sub={msgs.data ? `${msgs.data.filter((m) => m.enabled).length} ${t('enabled')}` : undefined} />
        <Stat label={t('Push devices')} value={fmtNum(push.data?.tokens ?? 0)} sub={push.data ? `${fmtNum(push.data.identities)} ${t('identities')} · ${t('provider')} ${push.data.mode}` : undefined} />
        <Stat label={t('Sessions, 24h')} value={fmtNum((auth.data?.buckets ?? []).reduce((a, b) => a + b.count, 0))} sub={t('$authenticate events')} />
      </div>
      <div className="card" style={{ marginTop: 16 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between' }}>
          <strong>{t('Authentications per hour, last 24h')}</strong>
          <Link to="/analytics" className="muted">{t('analytics →')}</Link>
        </div>
        <Sparkline points={(auth.data?.buckets ?? []).map((b) => b.count)} height={80} />
      </div>
    </Page>
  );
}

function UpdateBanner({ data, onDismiss }: { data: Updates; onDismiss: () => void }) {
  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <div className="row" style={{ justifyContent: 'space-between', alignItems: 'baseline', gap: 12, flexWrap: 'wrap' }}>
        <span>
          {t('Zanshin')} <strong>{data.latest}</strong> {t('is available')} · {t('you are running')} {data.current}
          {data.url && (
            <>
              {' '}· <a href={data.url} target="_blank" rel="noreferrer">{t('Release notes')}</a>
            </>
          )}
        </span>
        <button className="link" onClick={onDismiss}>{t('Dismiss')}</button>
      </div>
    </div>
  );
}

function GettingStarted({ data, onDismiss }: { data: Onboarding; onDismiss: () => void }) {
  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <div className="row" style={{ justifyContent: 'space-between', alignItems: 'baseline' }}>
        <h3 style={{ margin: 0 }}>{t('Getting started')}</h3>
        <span className="muted">
          {data.done} / {data.total} {t('done')} ·{' '}
          <button className="link" onClick={onDismiss}>{t('Dismiss')}</button>
        </span>
      </div>
      <ul className="checklist">
        {data.steps.map((st) => (
          <li key={st.id} className={st.done ? 'done' : ''}>
            <span className="mark">{st.done ? '✓' : '○'}</span>
            <div>
              {st.done ? st.title : <Link to={st.href}>{st.title}</Link>}
              {!st.done && <div className="muted small">{st.hint}</div>}
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}
