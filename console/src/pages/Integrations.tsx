import { useState } from 'react';
import { post } from '../api';
import { EntityPage, Text } from '../crud';
import { Badge, ErrorBox, Field, JsonField, Toggle, fmtDate } from '../ui';
import { t } from '../i18n';

type Kind = 'webhook' | 'onesignal' | 'fcm' | 'apns' | 'facebook';
type Health = { last_status: string | null; last_error: string | null; last_at: string | null; sent_count: number; failed_count: number };
type Integration = { id: string; key: string; name: string; kind: Kind; config: Record<string, unknown>; enabled: boolean; health: Health; version: number; updated_at: string };

const HINT: Record<Kind, string> = {
  webhook: 'config: url (required), secret (HMAC-SHA256 → X-Zanshin-Signature), headers {…}',
  onesignal: 'config: app_id, api_key (REST API key); players are addressed by external id',
  fcm: 'uses the server push.fcm config; delivers to the player\'s registered Android/web devices',
  apns: 'uses the server push.apns config; delivers to the player\'s registered iOS devices',
  facebook: 'config: access_token (app token); the player needs a facebook_id property',
};
const BLANK: Record<Kind, Record<string, unknown>> = {
  webhook: { url: '', secret: '' }, onesignal: { app_id: '', api_key: '' }, fcm: {}, apns: {}, facebook: { access_token: '' },
};

function TestSend({ i }: { i: Integration }) {
  const [ext, setExt] = useState('');
  const [out, setOut] = useState<{ status: string; detail: string } | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  return (
    <div className="card">
      <strong>{t('Test')}</strong>
      <ErrorBox error={err} />
      <div className="row" style={{ marginTop: 8, alignItems: 'end' }}>
        <Text label={t('Player (external id)')} value={ext} onChange={setExt} mono />
        <button className="btn" disabled={!ext} onClick={() => post<{ status: string; detail: string }>(`/integrations/${encodeURIComponent(i.key)}/test`, { external_id: ext }).then((r) => { setOut(r); setErr(null); }, setErr)}>{t('Send test message')}</button>
      </div>
      {out && <div style={{ marginTop: 8 }}><Badge tone={out.status === 'sent' ? 'ok' : out.status === 'failed' ? 'bad' : 'warn'}>{out.status}</Badge> <span className="hint">{out.detail}</span></div>}
    </div>
  );
}

function Health({ h }: { h?: Health }) {
  if (!h || !h.last_status) return <span className="hint">{t('never used')}</span>;
  const tone = h.last_status === 'failed' ? 'warn' : 'ok';
  return (
    <div>
      <Badge tone={tone}>{h.last_status}</Badge> <span className="hint">{fmtDate(h.last_at!)}</span>
      <div className="hint">{h.sent_count} sent · {h.failed_count} failed{h.last_error ? ` · ${h.last_error}` : ''}</div>
    </div>
  );
}

export default function Integrations() {
  return (
    <EntityPage<Integration>
      title={t('Integrations')}
      path="/integrations"
      blurb={t('Delivery channels beyond the inbox. Add an integration key to a campaign\'s channels and every delivery is also sent through it and logged per channel. Secrets are masked after saving.')}
      cols={[
        { h: 'Key', c: (i) => <><strong>{i.key}</strong><div className="hint">{i.name}</div></> },
        { h: 'Kind', c: (i) => <Badge tone="info">{i.kind}</Badge> },
        { h: 'Enabled', c: (i) => i.enabled ? <Badge tone="ok">on</Badge> : <Badge>off</Badge> },
        { h: 'Health', c: (i) => <Health h={i.health} /> },
        { h: 'Updated', c: (i) => fmtDate(i.updated_at) },
      ]}
      blank={() => ({ key: '', name: '', kind: 'webhook', config: { ...BLANK.webhook }, enabled: true })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label={t('Key')} value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label={t('Name')} value={d.name} onChange={(name) => set({ name })} />
            <div className="field fixed"><span className="label">{t('Enabled')}</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <Field label={t('Kind')} hint={HINT[d.kind ?? 'webhook']}>
            <select value={d.kind ?? 'webhook'} onChange={(e) => { const kind = e.target.value as Kind; set({ kind, config: isNew ? { ...BLANK[kind] } : d.config }); }}>{(Object.keys(HINT) as Kind[]).map((k) => <option key={k} value={k}>{k}</option>)}</select>
          </Field>
          <Field label={t('Config (JSON)')}><JsonField value={d.config} onChange={(config) => set({ config: (config as Record<string, unknown>) ?? {} })} rows={4} /></Field>
        </>
      )}
      extra={(i) => <TestSend i={i} />}
    />
  );
}
