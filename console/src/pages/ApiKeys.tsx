import { useState } from 'react';
import { del, get, post, useLoad } from '../api';
import { Badge, Confirm, ErrorBox, Field, Page, Table, fmtDate } from '../ui';

type Key = { id: string; name: string; kind: string; prefix: string; created_at: string; last_used_at: string | null; revoked_at: string | null };

export default function ApiKeys() {
  const q = useLoad(() => get<Key[]>('/api-keys'));
  const [name, setName] = useState('');
  const [kind, setKind] = useState('client');
  const [minted, setMinted] = useState<{ secret: string; name: string } | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      const r = await post<{ secret: string }>('/api-keys', { name, kind });
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
          { h: 'Kind', c: (k) => <Badge tone={k.kind === 'admin' ? 'bad' : k.kind === 'server' ? 'info' : 'muted'}>{k.kind}</Badge> },
          { h: "Prefix", c: (k) => <span className="mono">{k.prefix}…</span> },
          { h: 'Created', c: (k) => fmtDate(k.created_at) },
          { h: 'Last used', c: (k) => fmtDate(k.last_used_at) },
          { h: '', w: '120px', c: (k) => k.revoked_at ? <Badge tone="bad">revoked</Badge> : <Confirm danger onConfirm={() => del('/api-keys/' + k.id).then(q.reload, setErr)}>Revoke</Confirm> },
        ]}
      />
    </Page>
  );
}
