import { useState } from 'react';
import { del, get, post, api, useLoad } from '../api';
import { Badge, ErrorBox, Field, Menu, Page, Table, fmtDate } from '../ui';
import { PermissionEditor, type Permissions } from '../permissions';
import { useMe } from '../me';
import { toast } from '../toast';
import { t } from '../i18n';

type User = { id: string; email: string; name: string; permissions: Permissions; archetype: string | null; totp_enabled: boolean; disabled: boolean; must_change_password: boolean; created_at: string; last_login_at: string | null; recovery_codes_left: number; created_by: string | null };

function genPassword() {
  const a = 'abcdefghjkmnpqrstuvwxyzABCDEFGHJKMNPQRSTUVWXYZ23456789';
  const buf = new Uint32Array(20); crypto.getRandomValues(buf);
  return Array.from(buf, (n) => a[n % a.length]).join('');
}

export default function Users() {
  const me = useMe();
  const q = useLoad(() => get<User[]>('/users'));
  const [err, setErr] = useState<Error | null>(null);
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<User | null>(null);
  const [secret, setSecret] = useState<{ title: string; body: string } | null>(null);
  const resetLink = async (u: User) => {
    try {
      const r = await post<{ path: string }>(`/users/${u.id}/reset-link`);
      setSecret({ title: `Password reset link for ${u.email} (valid 24 h)`, body: `${window.location.origin}/console${r.path.replace(/^\/console/, '')}` });
    } catch (e) { setErr(e as Error); }
  };
  return (
    <Page title={t('Console users')} actions={<button className="btn" onClick={() => { setEditing(null); setCreating(true); }}>{t('New user')}</button>}>
      <ErrorBox error={q.error ?? err} />
      {secret && <div className="card"><strong>{secret.title}</strong> — shown once:<pre className="code" style={{ marginTop: 8 }}>{secret.body}</pre><button className="btn ghost" onClick={() => setSecret(null)}>{t('Done')}</button></div>}
      {(creating || editing) && (
        <UserForm
          user={editing}
          onClose={() => { setCreating(false); setEditing(null); }}
          onSaved={(info) => { setCreating(false); setEditing(null); q.reload(); if (info) setSecret(info); }}
        />
      )}
      <Table<User>
        rows={q.data}
        rowKey={(u) => u.id}
        empty={t('No console users yet — admins sign in with API keys or SSO until you create one.')}
        cols={[
          { h: 'User', c: (u) => <><strong>{u.name}</strong><div className="hint">{u.email}</div></> },
          { h: 'Access', c: (u) => <>{u.archetype ? <Badge tone="info">{u.archetype.replace('_', ' ')}</Badge> : <Badge>custom</Badge>}{u.disabled && <Badge tone="bad">disabled</Badge>}</> },
          { h: 'MFA', c: (u) => u.totp_enabled ? <Badge tone="ok">on · {u.recovery_codes_left} codes</Badge> : <Badge tone="warn">off</Badge> },
          { h: 'Password', c: (u) => <span className="hint">{u.must_change_password ? 'must change on login' : 'set'}</span> },
          { h: 'Last login', c: (u) => fmtDate(u.last_login_at) },
          { h: 'Created', c: (u) => <span className="hint">{fmtDate(u.created_at)}{u.created_by ? ` by ${u.created_by}` : ''}</span> },
          { h: '', w: '48px', c: (u) => (
            <Menu items={[
              { label: 'Edit access', onClick: () => { setCreating(false); setEditing(u); } },
              { label: 'Password reset link', onClick: () => void resetLink(u) },
              { label: u.disabled ? 'Enable' : 'Disable', onClick: () => api('PATCH', `/users/${u.id}`, { disabled: !u.disabled }).then(q.reload, setErr) },
              { label: 'Sign out everywhere', onClick: () => del(`/users/${u.id}/sessions`).then(() => toast(t('Sessions revoked'))).then(q.reload, setErr) },
              ...(u.totp_enabled ? [{ label: 'Remove MFA', onClick: () => del(`/users/${u.id}/mfa`).then(q.reload, setErr) }] : []),
              ...(me?.user_id !== u.id ? [{ label: 'Delete', danger: true, onClick: () => del(`/users/${u.id}`).then(q.reload, setErr) }] : []),
            ]} />
          ) },
        ]}
      />
    </Page>
  );
}

function UserForm({ user, onClose, onSaved }: { user: User | null; onClose: () => void; onSaved: (secret: { title: string; body: string } | null) => void }) {
  const [email, setEmail] = useState(user?.email ?? '');
  const [name, setName] = useState(user?.name ?? '');
  const [archetype, setArchetype] = useState<string | null>(user ? user.archetype : 'game_designer');
  const [permissions, setPermissions] = useState<Permissions>(user?.permissions ?? { '*': ['read'] });
  const [mode, setMode] = useState<'password' | 'link'>('password');
  const [password, setPassword] = useState(genPassword);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<Error | null>(null);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true); setErr(null);
    try {
      if (user) {
        await api('PATCH', `/users/${user.id}`, { name, archetype, permissions: archetype ? undefined : permissions });
        onSaved(null);
      } else {
        const r = await post<{ email: string; reset_token?: string }>('/users', { email, name, password: mode === 'password' ? password : undefined, archetype, permissions: archetype ? undefined : permissions });
        onSaved(mode === 'password'
          ? { title: `Initial password for ${r.email} (they must change it on first login)`, body: password }
          : { title: `Password setup link for ${r.email} (valid 24 h)`, body: `${window.location.origin}/console/reset?token=${r.reset_token}` });
      }
    } catch (e) { setErr(e as Error); } finally { setBusy(false); }
  };
  return (
    <form className="card stack" onSubmit={submit}>
      <h3 style={{ margin: 0 }}>{user ? `Edit ${user.email}` : 'New console user'}</h3>
      <ErrorBox error={err} />
      <div className="row">
        <Field label={t('Email')}><input type="email" value={email} onChange={(e) => setEmail(e.target.value)} required disabled={!!user} /></Field>
        <Field label={t('Name')}><input value={name} onChange={(e) => setName(e.target.value)} required /></Field>
      </div>
      {!user && (
        <div className="row">
          <Field label={t('First login')}>
            <select value={mode} onChange={(e) => setMode(e.target.value as typeof mode)}>
              <option value="password">{t('give them an initial password')}</option>
              <option value="link">{t('hand them a one-time setup link')}</option>
            </select>
          </Field>
          {mode === 'password' && <Field label={t('Initial password')} hint="min 12 characters; generated for you"><input value={password} onChange={(e) => setPassword(e.target.value)} className="mono" /></Field>}
        </div>
      )}
      <Field label={t('Permissions')} hint="a preset or an explicit resource × action grid; changes sign the user out of active sessions">
        <PermissionEditor archetype={archetype} permissions={permissions} onChange={(a, p) => { setArchetype(a); setPermissions(p); }} />
      </Field>
      <div className="row">
        <button className="btn" disabled={busy}>{user ? 'Save' : 'Create user'}</button>
        <button type="button" className="btn ghost" onClick={onClose}>{t('Cancel')}</button>
      </div>
    </form>
  );
}
