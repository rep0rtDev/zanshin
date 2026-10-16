import { useState } from 'react';
import { del, get, post, useLoad } from '../api';
import { Badge, Confirm, ErrorBox, Field, Page, Table, fmtDate } from '../ui';
import { PermissionEditor, type Permissions } from '../permissions';
import { t, tf } from '../i18n';

type Key = { id: string; name: string; kind: string; prefix: string; created_at: string; last_used_at: string | null; revoked_at: string | null; read_only: boolean; role: 'admin' | 'editor' | 'viewer'; expires_at: string | null; rate_limit_per_minute: number | null; permissions: Permissions | null; rotated_from: string | null; rotated_to: string | null };

export default function ApiKeys() {
  const q = useLoad(() => get<Key[]>('/api-keys'));
  const [name, setName] = useState('');
  const [kind, setKind] = useState('client');
  const [archetype, setArchetype] = useState<string | null>('admin');
  const [permissions, setPermissions] = useState<Permissions>({ '*': ['read', 'write', 'delete'] });
  const [expires, setExpires] = useState('');
  const [limit, setLimit] = useState('');
  const [minted, setMinted] = useState<{ secret: string; name: string } | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      const r = await post<{ secret: string }>('/api-keys', { name, kind, archetype: kind === 'admin' ? archetype : null, permissions: kind === 'admin' && !archetype ? permissions : null, expires_at: expires ? new Date(expires).toISOString() : null, rate_limit_per_minute: limit ? Number(limit) : null });
      setMinted({ secret: r.secret, name });
      setName('');
      q.reload();
    } catch (e) { setErr(e as Error); }
  };
  return (
    <Page title={t('API keys')}>
      <ErrorBox error={q.error ?? err} />
      <form className="card" onSubmit={create}>
       <div className="row">
        <Field label={t('Name')}><input value={name} onChange={(e) => setName(e.target.value)} placeholder="ios-prod" required /></Field>
        <Field label={t('Kind')}>
          <select value={kind} onChange={(e) => setKind(e.target.value)}>
            <option value="client">{t('client — game clients (/v1)')}</option>
            <option value="server">{t('server — trusted backends, Nakama (/server/v1)')}</option>
            <option value="admin">{t('admin — this console (/admin/v1)')}</option>
          </select>
        </Field>
        <Field label={t('Expires')} hint="optional"><input type="datetime-local" value={expires} onChange={(e) => setExpires(e.target.value)} /></Field>
        <Field label={t('Requests / min')} hint={t('optional per-key quota')}><input type="number" min={1} value={limit} onChange={(e) => setLimit(e.target.value)} placeholder={t('scope default')} /></Field>
        <div className="field fixed"><span className="label">&nbsp;</span><button className="btn">{t('Create key')}</button></div>
       </div>
        {kind === 'admin' && <Field label={t('Permissions')} hint={t('what this admin key may do in /admin/v1')}><PermissionEditor archetype={archetype} permissions={permissions} onChange={(a, p) => { setArchetype(a); setPermissions(p); }} /></Field>}
      </form>
      {minted && (
        <div className="card">
          <strong>{minted.name}</strong> — copy the secret now, it is shown once:
          <pre className="code" style={{ marginTop: 8 }}>{minted.secret}</pre>
        </div>
      )}
      <Table<Key>
        rows={q.data}
        rowKey={(k) => k.id}
        cols={[
          { h: 'Name', c: (k) => <strong>{k.name}</strong> },
          { h: 'Kind', c: (k) => <><Badge tone={k.kind === 'admin' ? 'bad' : k.kind === 'server' ? 'info' : 'muted'}>{k.kind}</Badge>{k.kind === 'admin' && (k.permissions ? <Badge>{t('custom perms')}</Badge> : k.role !== 'admin' && <Badge>{k.role}</Badge>)}</> },
          { h: 'Limits', c: (k) => <span className="hint">{k.rate_limit_per_minute ? `${k.rate_limit_per_minute}/min` : 'default'}{k.expires_at ? tf(' · expires {0}', fmtDate(k.expires_at)) : ''}</span> },
          { h: "Prefix", c: (k) => <span className="mono">{k.prefix}…</span> },
          { h: 'Created', c: (k) => fmtDate(k.created_at) },
          { h: 'Last used', c: (k) => fmtDate(k.last_used_at) },
          { h: '', w: '200px', c: (k) => k.revoked_at ? <Badge tone="bad">revoked</Badge> : k.rotated_to ? <Badge tone="warn">rotated · expires {fmtDate(k.expires_at)}</Badge> : <span className="row" style={{ gap: 6 }}><Confirm onConfirm={() => post<{ secret: string }>('/api-keys/' + k.id + '/rotate', { grace_hours: 24 }).then((r) => { setMinted({ secret: r.secret, name: k.name + ' (rotated — old key works 24 h)' }); q.reload(); }, setErr)}>{t('Rotate')}</Confirm><Confirm danger onConfirm={() => del('/api-keys/' + k.id).then(q.reload, setErr)}>{t('Revoke')}</Confirm></span> },
        ]}
      />
    </Page>
  );
}
