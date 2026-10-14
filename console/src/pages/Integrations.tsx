import { useState } from 'react';
import { post } from '../api';
import { EntityPage, Text } from '../crud';
import { Badge, ErrorBox, Field, JsonField, Toggle, fmtDate } from '../ui';

type Kind = 'webhook' | 'onesignal' | 'fcm' | 'apns' | 'facebook';
type Integration = { id: string; key: string; name: string; kind: Kind; config: Record<string, unknown>; enabled: boolean; version: number; updated_at: string };

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
      <strong>Test</strong>
      <ErrorBox error={err} />
      <div className="row" style={{ marginTop: 8, alignItems: 'end' }}>
        <Text label="Player (external id)" value={ext} onChange={setExt} mono />
        <button className="btn" disabled={!ext} onClick={() => post<{ status: string; detail: string }>(`/integrations/${encodeURIComponent(i.key)}/test`, { external_id: ext }).then((r) => { setOut(r); setErr(null); }, setErr)}>Send test message</button>
      </div>
      {out && <div style={{ marginTop: 8 }}><Badge tone={out.status === 'sent' ? 'ok' : out.status === 'failed' ? 'bad' : 'warn'}>{out.status}</Badge> <span className="hint">{out.detail}</span></div>}
    </div>
  );
}

export default function Integrations() {
  return (
    <EntityPage<Integration>
      title="Integrations"
      path="/integrations"
      blurb="Delivery channels beyond the inbox. Add an integration key to a campaign's channels and every delivery is also sent through it and logged per channel. Secrets are masked after saving."
      cols={[
        { h: 'Key', c: (i) => <><strong>{i.key}</strong><div className="hint">{i.name}</div></> },
        { h: 'Kind', c: (i) => <Badge tone="info">{i.kind}</Badge> },
        { h: 'Enabled', c: (i) => i.enabled ? <Badge tone="ok">on</Badge> : <Badge>off</Badge> },
        { h: 'Updated', c: (i) => fmtDate(i.updated_at) },
      ]}
      blank={() => ({ key: '', name: '', kind: 'webhook', config: { ...BLANK.webhook }, enabled: true })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} />
            <div className="field fixed"><span className="label">Enabled</span><Toggle value={!!d.enabled} onChange={(enabled) => set({ enabled })} label={d.enabled ? 'on' : 'off'} /></div>
          </div>
          <Field label="Kind" hint={HINT[d.kind ?? 'webhook']}>
            <select value={d.kind ?? 'webhook'} onChange={(e) => { const kind = e.target.value as Kind; set({ kind, config: isNew ? { ...BLANK[kind] } : d.config }); }}>{(Object.keys(HINT) as Kind[]).map((k) => <option key={k} value={k}>{k}</option>)}</select>
          </Field>
          <Field label="Config (JSON)"><JsonField value={d.config} onChange={(config) => set({ config: (config as Record<string, unknown>) ?? {} })} rows={4} /></Field>
        </>
      )}
      extra={(i) => <TestSend i={i} />}
    />
  );
}
