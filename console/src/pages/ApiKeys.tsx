import { useState } from 'react';
import { del, get, post, useLoad } from '../api';
import { Badge, Confirm, ErrorBox, Field, Page, Table, fmtDate } from '../ui';

type Key = { id: string; name: string; kind: string; prefix: string; created_at: string; last_used_at: string | null; revoked_at: string | null; read_only: boolean; role: 'admin' | 'editor' | 'viewer'; expires_at: string | null; rate_limit_per_minute: number | null };

export default function ApiKeys() {
  const q = useLoad(() => get<Key[]>('/api-keys'));
  const [name, setName] = useState('');
  const [kind, setKind] = useState('client');
  const [role, setRole] = useState<'admin' | 'editor' | 'viewer'>('admin');
  const [expires, setExpires] = useState('');
  const [limit, setLimit] = useState('');
  const [minted, setMinted] = useState<{ secret: string; name: string } | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      const r = await post<{ secret: string }>('/api-keys', { name, kind, role: kind === 'admin' ? role : null, expires_at: expires ? new Date(expires).toISOString() : null, rate_limit_per_minute: limit ? Number(limit) : null });
      setMinted({ secret: r.secret, name });
      setName('');
      q.reload();
    } catch (e) { setErr(e as Error); }
  };
  return (
    <Page title="API keys">
      <ErrorBox error={q.error ?? err} />
      <form className="card row" onSubmit={create}>
        <Field label="Name"><input value={name} onChange={(e) => setName(e.target.value)} placeholder="ios-prod" required /></Field>
        <Field label="Kind">
          <select value={kind} onChange={(e) => setKind(e.target.value)}>
            <option value="client">client — game clients (/v1)</option>
            <option value="server">server — trusted backends, Nakama (/server/v1)</option>
            <option value="admin">admin — this console (/admin/v1)</option>
          </select>
        </Field>
        <Field label="Expires" hint="optional"><input type="datetime-local" value={expires} onChange={(e) => setExpires(e.target.value)} /></Field>
        <Field label="Requests / min" hint="optional per-key quota"><input type="number" min={1} value={limit} onChange={(e) => setLimit(e.target.value)} placeholder="scope default" /></Field>
        {kind === 'admin' && <label className="field fixed"><span className="label">Role</span><select value={role} onChange={(e) => setRole(e.target.value as typeof role)}><option value="admin">admin — everything</option><option value="editor">editor — no key management</option><option value="viewer">viewer — read only</option></select></label>}
        <div className="field fixed"><span className="label">&nbsp;</span><button className="btn">Create key</button></div>
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
          { h: 'Kind', c: (k) => <><Badge tone={k.kind === 'admin' ? 'bad' : k.kind === 'server' ? 'info' : 'muted'}>{k.kind}</Badge>{k.kind === 'admin' && k.role !== 'admin' && <Badge>{k.role}</Badge>}</> },
          { h: 'Limits', c: (k) => <span className="hint">{k.rate_limit_per_minute ? `${k.rate_limit_per_minute}/min` : 'default'}{k.expires_at ? ` · expires ${fmtDate(k.expires_at)}` : ''}</span> },
          { h: "Prefix", c: (k) => <span className="mono">{k.prefix}…</span> },
          { h: 'Created', c: (k) => fmtDate(k.created_at) },
          { h: 'Last used', c: (k) => fmtDate(k.last_used_at) },
          { h: '', w: '120px', c: (k) => k.revoked_at ? <Badge tone="bad">revoked</Badge> : <Confirm danger onConfirm={() => del('/api-keys/' + k.id).then(q.reload, setErr)}>Revoke</Confirm> },
        ]}
      />
    </Page>
  );
}
