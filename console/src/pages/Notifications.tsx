import { useState } from 'react';
import { get, post, useLoad } from '../api';
import { EntityPage, Tags, Text } from '../crud';
import { Badge, ErrorBox, Field, JsonField, fmtDate } from '../ui';
import { t } from '../i18n';

type Channel = {
  id: string; key: string; name: string; kind: 'slack' | 'discord' | 'webhook'; url: string; topics: string[]; kinds: string[]; enabled: boolean;
  headers: Record<string, unknown>; last_status: number | null; last_error: string; last_sent_at: string | null; tags: string[]; version: number; updated_at: string;
};

const KINDS = ['flags', 'audiences', 'experiments', 'live-events', 'messages', 'templates', 'journeys', 'metrics', 'webhooks', 'integrations', 'export-sinks', 'taxonomy', 'api-keys', 'users'];

function TopicsPicker({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const q = useLoad(() => get<{ items: string[] }>('/notification-channels/topics'), []);
  const toggle = (t: string) => onChange(value.includes(t) ? value.filter((x) => x !== t) : [...value.filter((x) => x !== '*'), t]);
  return (
    <div className="chips">
      <button type="button" className={'chip' + (value.length === 0 || value.includes('*') ? ' on' : '')} onClick={() => onChange(['*'])}>* everything</button>
      {(q.data?.items ?? []).map((t) => <button key={t} type="button" className={'chip' + (value.includes(t) ? ' on' : '')} onClick={() => toggle(t)}>{t}</button>)}
    </div>
  );
}

function TestSend({ c }: { c: Channel }) {
  const [res, setRes] = useState<{ ok: boolean; status: number | null; error: string } | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}>
        <strong>{t('Delivery')}</strong>
        <span className="hint">{c.last_sent_at ? `last sent ${fmtDate(c.last_sent_at)}` : 'never sent'}{c.last_status ? ` · HTTP ${c.last_status}` : ''}</span>
        {c.last_error && <Badge tone="bad">{c.last_error}</Badge>}
        <button className="btn sm" style={{ marginLeft: 'auto' }} onClick={() => post<{ ok: boolean; status: number | null; error: string }>(`/notification-channels/${encodeURIComponent(c.key)}/test`).then((r) => { setRes(r); setErr(null); }, setErr)}>{t('Send test message')}</button>
      </div>
      <ErrorBox error={err} />
      {res && <div style={{ marginTop: 8 }}><Badge tone={res.ok ? 'ok' : 'bad'}>{res.ok ? 'delivered' : 'failed'}</Badge> <span className="hint">{res.status ? `HTTP ${res.status}` : ''} {res.error}</span></div>}
    </div>
  );
}

export default function Notifications() {
  return (
    <EntityPage<Channel>
      title={t('Notifications')}
      path="/notification-channels"
      blurb={t('Post configuration changes, failed export runs, unhealthy integrations and alerts to Slack, Discord or any webhook.')}
      cols={[
        { h: 'Key', c: (c) => <><strong>{c.key}</strong><div className="hint">{c.name}</div></> },
        { h: 'Kind', c: (c) => <code>{c.kind}</code> },
        { h: 'Topics', c: (c) => (c.topics.length === 0 ? '*' : c.topics.join(', ')) },
        { h: 'Status', c: (c) => (c.last_error ? <Badge tone="bad">error</Badge> : c.last_sent_at ? <Badge tone="ok">ok</Badge> : <span className="hint">—</span>) },
        { h: 'Enabled', c: (c) => (c.enabled ? 'yes' : 'no') },
      ]}
      blank={() => ({ key: '', name: '', kind: 'slack', url: '', topics: ['*'], kinds: [], enabled: true, headers: {}, tags: [] })}
      form={(c, set, isNew) => (
        <>
          <div className="row"><Text label={t('Key')} value={c.key} onChange={(key) => set({ key })} disabled={!isNew} mono /><Text label={t('Name')} value={c.name} onChange={(name) => set({ name })} /></div>
          <div className="row">
            <Field label={t('Kind')}><select value={c.kind ?? 'slack'} onChange={(e) => set({ kind: e.target.value as Channel['kind'] })}><option value="slack">{t('Slack incoming webhook')}</option><option value="discord">{t('Discord webhook')}</option><option value="webhook">{t('Generic webhook')}</option></select></Field>
            <Text label={t('URL')} value={c.url} onChange={(url) => set({ url })} mono placeholder={c.kind === 'discord' ? 'https://discord.com/api/webhooks/…' : 'https://hooks.slack.com/services/…'} />
            <Field label={t('Enabled')}><input type="checkbox" checked={c.enabled ?? true} onChange={(e) => set({ enabled: e.target.checked })} /></Field>
          </div>
          <Field label={t('Topics')}><TopicsPicker value={c.topics ?? []} onChange={(topics) => set({ topics })} /></Field>
          <Field label="Only config changes of these kinds (empty = all)">
            <div className="chips">{KINDS.map((k) => <button key={k} type="button" className={'chip' + ((c.kinds ?? []).includes(k) ? ' on' : '')} onClick={() => set({ kinds: (c.kinds ?? []).includes(k) ? (c.kinds ?? []).filter((x) => x !== k) : [...(c.kinds ?? []), k] })}>{k}</button>)}</div>
          </Field>
          {c.kind === 'webhook' && <Field label={t('Extra headers (JSON object)')}><JsonField value={c.headers ?? {}} onChange={(headers) => set({ headers: headers as Record<string, unknown> })} rows={2} /></Field>}
          <Tags value={c.tags ?? []} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(c) => <TestSend c={c} />}
    />
  );
}
