import { useEffect, useState } from 'react';
import QRCode from 'qrcode';
import { del, get, post, useLoad, put } from '../api';
import { Badge, ErrorBox, Field, Page, Table, fmtDate } from '../ui';
import { useMe } from '../me';
import { toast } from '../toast';
import { PermissionMatrix, useCatalog } from '../permissions';
import { LOCALES, locale, setLocale, t, type Locale } from '../i18n';

type Session = { id: string; created_at: string; expires_at: string; last_seen_at: string; ip: string | null; user_agent: string | null };

export default function Account({ onChanged }: { onChanged: () => void }) {
  const me = useMe();
  const cat = useCatalog();
  const isUser = me?.auth === 'user';
  const sessions = useLoad(() => (isUser ? get<Session[]>('/me/sessions') : Promise.resolve([] as Session[])), [isUser]);
  const [err, setErr] = useState<Error | null>(null);
  const [cur, setCur] = useState(''); const [nw, setNw] = useState(''); const [nw2, setNw2] = useState('');
  const [setup, setSetup] = useState<{ secret: string; otpauth_url: string; qr?: string } | null>(null);
  const [code, setCode] = useState('');
  const [recovery, setRecovery] = useState<string[] | null>(null);
  useEffect(() => {
    if (setup && !setup.qr) QRCode.toDataURL(setup.otpauth_url, { margin: 1, width: 180 }).then((qr) => setSetup((s) => (s ? { ...s, qr } : s))).catch(() => undefined);
  }, [setup]);
  const changePassword = async (e: React.FormEvent) => {
    e.preventDefault();
    if (nw !== nw2) { setErr(new Error('New passwords do not match')); return; }
    try {
      await post('/me/password', { current_password: cur, new_password: nw });
      toast(t('Password changed — sign in again'));
      onChanged();
    } catch (e) { setErr(e as Error); }
  };
  const enable = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      const r = await post<{ recovery_codes: string[] }>('/me/mfa/enable', { code });
      setRecovery(r.recovery_codes); setSetup(null); setCode(''); onChanged();
    } catch (e) { setErr(e as Error); }
  };
  const disable = async (e: React.FormEvent) => {
    e.preventDefault();
    try { await post('/me/mfa/disable', { code }); setCode(''); toast(t('Two-factor authentication disabled'), 'info'); onChanged(); } catch (e) { setErr(e as Error); }
  };
  if (!me) return null;
  return (
    <Page title={t('Your account')}>
      <ErrorBox error={err} />
      <div className="card stack">
        <label className="field fixed"><span className="label">{t('Language')}</span><select value={locale} onChange={(e) => { const l = e.target.value as Locale; (isUser ? put('/me/locale', { locale: l }) : Promise.resolve()).finally(() => setLocale(l)); }}>{LOCALES.map(([k, n]) => <option key={k} value={k}>{n}</option>)}</select></label>
        <div><strong>{me.name}</strong> <Badge tone="info">{me.auth === 'user' ? 'password login' : me.auth === 'oidc' ? 'single sign-on' : me.auth === 'bootstrap' ? 'bootstrap key' : 'API key'}</Badge> {me.mfa_enabled && <Badge tone="ok">{t('MFA on')}</Badge>}</div>
        {cat && <PermissionMatrix value={me.permissions} onChange={() => undefined} resources={cat.resources} actions={cat.actions} disabled />}
      </div>
      {isUser && (
        <>
          <form className="card stack" onSubmit={changePassword}>
            <h3 style={{ margin: 0 }}>{t('Change password')}</h3>
            {me.must_change_password && <div className="badge warn">{t('You were given an initial password — set your own now.')}</div>}
            <div className="row">
              <Field label={t('Current password')}><input type="password" value={cur} onChange={(e) => setCur(e.target.value)} required autoComplete="current-password" /></Field>
              <Field label={t('New password')} hint={t('at least 12 characters')}><input type="password" value={nw} onChange={(e) => setNw(e.target.value)} required autoComplete="new-password" /></Field>
              <Field label={t('Repeat')}><input type="password" value={nw2} onChange={(e) => setNw2(e.target.value)} required autoComplete="new-password" /></Field>
            </div>
            <div><button className="btn">{t('Change password')}</button></div>
          </form>
          <div className="card stack">
            <h3 style={{ margin: 0 }}>{t('Two-factor authentication')}</h3>
            {recovery && (
              <div className="stack">
                <div className="badge ok">{t('Enabled. Store these recovery codes — each works once if you lose the authenticator:')}</div>
                <pre className="code">{recovery.join('\n')}</pre>
                <div><button className="btn ghost" onClick={() => setRecovery(null)}>{t('I saved them')}</button></div>
              </div>
            )}
            {!me.mfa_enabled && !setup && !recovery && <div><button className="btn" onClick={() => post<{ secret: string; otpauth_url: string }>('/me/mfa/setup').then(setSetup, setErr)}>{t('Set up authenticator app')}</button></div>}
            {setup && (
              <form className="row" onSubmit={enable} style={{ alignItems: 'flex-start' }}>
                {setup.qr ? <img src={setup.qr} alt="TOTP QR code" width={180} height={180} style={{ borderRadius: 8, background: '#fff' }} /> : <div className="hint">{t('Rendering QR…')}</div>}
                <div className="stack">
                  <div className="hint">{t('Scan with Google Authenticator / 1Password / Authy, or enter the secret manually:')}</div>
                  <pre className="code">{setup.secret}</pre>
                  <Field label={t('Code from the app')}><input value={code} onChange={(e) => setCode(e.target.value)} inputMode="numeric" pattern="[0-9]{6}" required autoFocus /></Field>
                  <div className="row"><button className="btn">{t('Enable')}</button><button type="button" className="btn ghost" onClick={() => setSetup(null)}>{t('Cancel')}</button></div>
                </div>
              </form>
            )}
            {me.mfa_enabled && !recovery && (
              <form className="row" onSubmit={disable}>
                <Field label={t('Current code')} hint={t('required to turn MFA off')}><input value={code} onChange={(e) => setCode(e.target.value)} inputMode="numeric" required /></Field>
                <div className="field fixed"><span className="label">&nbsp;</span><button className="btn danger">{t('Disable MFA')}</button></div>
              </form>
            )}
          </div>
          <div className="card stack">
            <h3 style={{ margin: 0 }}>{t('Active sessions')}</h3>
            <Table<Session>
              rows={sessions.data}
              rowKey={(s) => s.id}
              cols={[
                { h: 'Started', c: (s) => fmtDate(s.created_at) },
                { h: 'Last seen', c: (s) => fmtDate(s.last_seen_at) },
                { h: 'Expires', c: (s) => fmtDate(s.expires_at) },
                { h: 'From', c: (s) => <span className="hint">{s.ip ?? '—'} · {s.user_agent?.slice(0, 60) ?? '—'}</span> },
                { h: '', w: '100px', c: (s) => <button className="btn ghost" onClick={() => del(`/me/sessions/${s.id}`).then(sessions.reload, setErr)}>{t('Revoke')}</button> },
              ]}
            />
          </div>
        </>
      )}
    </Page>
  );
}
