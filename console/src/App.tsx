import { useCallback, useEffect, useState } from 'react';
import { NavLink, Navigate, Route, Routes, useLocation } from 'react-router-dom';
import { Palette, type PaletteItem } from './Palette';
import { Toasts } from './toast';
import Settings from './pages/Settings';
import { MeContext } from './me';
import { ApiError, auth, authConfig, get, passwordLogin, resetPassword, ssoLogout, type AuthConfig, type Me } from './api';
import Users from './pages/Users';
import Account from './pages/Account';
import { allows } from './permissions';
import { toast } from './toast';
import Dashboard from './pages/Dashboard';
import Flags from './pages/Flags';
import Experiments from './pages/Experiments';
import Audiences from './pages/Audiences';
import LiveEvents from './pages/LiveEvents';
import Messages from './pages/Messages';
import Identities from './pages/Identities';
import Analytics from './pages/Analytics';
import Taxonomy from './pages/Taxonomy';
import Metrics from './pages/Metrics';
import Templates from './pages/Templates';
import Integrations from './pages/Integrations';
import Webhooks from './pages/Webhooks';
import Journeys from './pages/Journeys';
import Insights from './pages/Insights';
import Data from './pages/Data';
import ApiKeys from './pages/ApiKeys';
import Audit from './pages/Audit';

function Login({ onDone, cfg }: { onDone: () => void; cfg: AuthConfig }) {
  const params = new URLSearchParams(window.location.search);
  const resetToken = window.location.pathname.endsWith('/reset') ? params.get('token') : null;
  const defaultTab = resetToken ? 'reset' : cfg.password ? 'password' : cfg.needs_bootstrap ? 'bootstrap' : 'key';
  const [tab, setTabState] = useState<'password' | 'key' | 'bootstrap' | 'reset'>(defaultTab);
  const [touched, setTouched] = useState(false);
  const setTab = (t: typeof tab) => { setTouched(true); setTabState(t); };
  useEffect(() => { if (!touched) setTabState(defaultTab); }, [defaultTab, touched]);
  const [key, setKey] = useState('');
  const [email, setEmail] = useState(''); const [password, setPassword] = useState(''); const [code, setCode] = useState('');
  const [needCode, setNeedCode] = useState(false);
  const [name, setName] = useState('');
  const [err, setErr] = useState<string | null>(() => params.get('error'));
  const [busy, setBusy] = useState(false);
  const run = async (fn: () => Promise<void>) => { setBusy(true); setErr(null); try { await fn(); } catch (e) { setErr((e as Error).message || 'Failed'); } finally { setBusy(false); } };
  const withKey = (e: React.FormEvent) => { e.preventDefault(); void run(async () => { auth.set(key.trim()); try { await get('/me'); onDone(); } catch (e) { auth.clear(); throw e; } }); };
  const withPassword = (e: React.FormEvent) => { e.preventDefault(); void run(async () => {
    try { await passwordLogin(email, password, code); auth.clear(); onDone(); }
    catch (e) { if (e instanceof ApiError && e.code === 'mfa_required') { setNeedCode(true); return; } throw e; }
  }); };
  const bootstrap = (e: React.FormEvent) => { e.preventDefault(); void run(async () => {
    auth.set(key.trim());
    try { await get('/me'); } catch (e) { auth.clear(); throw e; }
    await (await import('./api')).post('/users', { email, name, password, archetype: 'admin' });
    auth.clear();
    await passwordLogin(email, password);
    onDone();
  }); };
  const reset = (e: React.FormEvent) => { e.preventDefault(); void run(async () => { await resetPassword(resetToken!, password); window.history.replaceState(null, '', '/console/'); setTab('password'); setPassword(''); toast('Password set — sign in'); }); };
  const tabs: [typeof tab, string][] = [];
  if (cfg.password) tabs.push(['password', 'Email & password']);
  if (cfg.needs_bootstrap) tabs.push(['bootstrap', 'Create first admin']);
  tabs.push(['key', 'Admin API key']);
  return (
    <div className="login">
      <div className="card">
        <div className="brand">Zanshin<small>admin console</small></div>
        {cfg.oidc && (<><a className="btn" href="/console/auth/login" style={{ width: '100%', display: 'block', textAlign: 'center', boxSizing: 'border-box' }}>Sign in with SSO</a><div className="hint" style={{ textAlign: 'center', margin: '12px 0' }}>or</div></>)}
        {tab !== 'reset' && tabs.length > 1 && <div className="tabs" style={{ marginBottom: 12 }}>{tabs.map(([t, l]) => <button key={t} type="button" className={'tab' + (tab === t ? ' active' : '')} onClick={() => { setTab(t); setErr(null); }}>{l}</button>)}</div>}
        {tab === 'password' && (
          <form onSubmit={withPassword}>
            <label className="field"><span className="label">Email</span><input type="email" autoFocus value={email} onChange={(e) => setEmail(e.target.value)} required autoComplete="username" disabled={needCode} /></label>
            <label className="field"><span className="label">Password</span><input type="password" value={password} onChange={(e) => setPassword(e.target.value)} required autoComplete="current-password" disabled={needCode} /></label>
            {needCode && <label className="field"><span className="label">One-time code</span><input autoFocus value={code} onChange={(e) => setCode(e.target.value)} inputMode="numeric" required autoComplete="one-time-code" /><span className="hint">From your authenticator app, or a recovery code.</span></label>}
            {err && <div className="error">{err}</div>}
            <button className="btn" disabled={busy} style={{ width: '100%' }}>{needCode ? 'Verify' : 'Sign in'}</button>
            <div className="hint" style={{ marginTop: 10 }}>Forgot the password? Ask an admin for a reset link.</div>
          </form>
        )}
        {tab === 'bootstrap' && (
          <form onSubmit={bootstrap}>
            <div className="hint" style={{ marginBottom: 10 }}>No console users exist yet. Use the bootstrap admin key from the server config once to create the first admin account.</div>
            <label className="field"><span className="label">Bootstrap admin key</span><input type="password" autoFocus value={key} onChange={(e) => setKey(e.target.value)} required /></label>
            <label className="field"><span className="label">Your name</span><input value={name} onChange={(e) => setName(e.target.value)} required /></label>
            <label className="field"><span className="label">Email</span><input type="email" value={email} onChange={(e) => setEmail(e.target.value)} required /></label>
            <label className="field"><span className="label">Password</span><input type="password" value={password} onChange={(e) => setPassword(e.target.value)} required minLength={12} autoComplete="new-password" /><span className="hint">At least 12 characters.</span></label>
            {err && <div className="error">{err}</div>}
            <button className="btn" disabled={busy} style={{ width: '100%' }}>Create admin & sign in</button>
          </form>
        )}
        {tab === 'key' && (
          <form onSubmit={withKey}>
            <label className="field">
              <span className="label">Admin API key</span>
              <input type="password" autoFocus value={key} onChange={(e) => setKey(e.target.value)} placeholder="zk_a_…" />
              <span className="hint">The bootstrap key from config or an admin key minted via /admin/v1/api-keys. Stored in this browser only.</span>
            </label>
            {err && <div className="error">{err}</div>}
            <button className="btn" disabled={busy} style={{ width: '100%' }}>Sign in</button>
          </form>
        )}
        {tab === 'reset' && (
          <form onSubmit={reset}>
            <div className="hint" style={{ marginBottom: 10 }}>Choose a new password for your account.</div>
            <label className="field"><span className="label">New password</span><input type="password" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} required minLength={12} autoComplete="new-password" /></label>
            {err && <div className="error">{err}</div>}
            <button className="btn" disabled={busy} style={{ width: '100%' }}>Set password</button>
          </form>
        )}
      </div>
    </div>
  );
}

type NavGroup = { name: string; items: [string, string, string][] };
const I = {
  home: 'M3 11l9-8 9 8v9a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z', flag: 'M5 21V4h10l-1 3h5l-2 5 2 5h-7l1-3H7v7z', users: 'M16 21v-2a4 4 0 0 0-8 0v2M12 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM21 21v-2a4 4 0 0 0-3-3.9M17 3.1a4 4 0 0 1 0 7.8',
  lab: 'M9 3h6M10 3v6l-5 9a2 2 0 0 0 2 3h10a2 2 0 0 0 2-3l-5-9V3', cal: 'M4 5h16v16H4zM4 10h16M8 3v4M16 3v4', mail: 'M3 6h18v12H3zM3 7l9 6 9-6', doc: 'M6 2h8l5 5v15H6zM14 2v6h5',
  route: 'M5 19a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM19 9a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM7 17h7a3 3 0 0 0 0-6H10a3 3 0 0 1 0-6h7', plug: 'M9 2v6M15 2v6M6 8h12v4a6 6 0 0 1-12 0zM12 18v4',
  tag: 'M3 3h8l10 10-8 8L3 11zM8 8h.01', chart: 'M4 20V10M10 20V4M16 20v-7M22 20H2', pulse: 'M2 12h4l3-8 4 16 3-8h6', gauge: 'M12 14l4-6M4 18a9 9 0 1 1 16 0',
  hook: 'M18 16a4 4 0 1 1-4-4h4V4M4 4h4v4', db: 'M12 3c5 0 8 1.3 8 3s-3 3-8 3-8-1.3-8-3 3-3 8-3zM4 6v12c0 1.7 3 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3 3 8 3s8-1.3 8-3',
  key: 'M14 3a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM13 13L3 23M6 20l2 2M9 17l2 2', list: 'M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01', gear: 'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z',
};
const Icon = ({ d }: { d: string }) => <svg className="ico" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d={d} /></svg>;

const GROUPS: NavGroup[] = [
  { name: '', items: [['/', 'Overview', I.home]] },
  { name: 'Targeting', items: [['/flags', 'Feature flags', I.flag], ['/audiences', 'Audiences', I.users], ['/experiments', 'Experiments', I.lab], ['/live-events', 'Live events', I.cal]] },
  { name: 'Engage', items: [['/messages', 'Messages', I.mail], ['/templates', 'Templates', I.doc], ['/journeys', 'Journeys', I.route], ['/integrations', 'Integrations', I.plug]] },
  { name: 'Players', items: [['/identities', 'Identities', I.users], ['/taxonomy', 'Taxonomy', I.tag]] },
  { name: 'Analytics', items: [['/insights', 'Insights', I.chart], ['/analytics', 'Events', I.pulse], ['/metrics', 'Metrics', I.gauge]] },
  { name: 'Platform', items: [['/webhooks', 'Webhooks', I.hook], ['/data', 'Data', I.db], ['/api-keys', 'API keys', I.key], ['/users', 'Users', I.users], ['/audit', 'Audit log', I.list], ['/settings', 'Settings', I.gear]] },
];

const THEME_KEY = 'zanshin.theme';
function initialTheme(): 'light' | 'dark' {
  const saved = localStorage.getItem(THEME_KEY);
  if (saved === 'light' || saved === 'dark') return saved;
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

function Crumbs() {
  const { pathname } = useLocation();
  const parts = pathname.split('/').filter(Boolean);
  const item = GROUPS.flatMap((g) => g.items.map((it) => [g.name, it] as const)).find(([, it]) => it[0] === '/' + (parts[0] ?? ''));
  return (
    <div className="crumbs">
      {item?.[0] && <span className="muted">{item[0]} /</span>}
      <span>{item?.[1][1] ?? (parts[0] === 'account' ? 'Your account' : 'Overview')}</span>
      {parts.slice(1).map((p, i) => <span key={i} className="muted">/ {decodeURIComponent(p)}</span>)}
    </div>
  );
}

export default function App() {
  const [me, setMe] = useState<Me | null | undefined>(undefined);
  const [cfg, setCfg] = useState<AuthConfig>({ oidc: false, password: false, needs_bootstrap: false });
  const [palette, setPalette] = useState(false);
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem('zanshin.nav') === 'collapsed');
  const loadMe = useCallback(async () => {
    try {
      setMe(await get<Me>('/me'));
    } catch {
      setMe(null);
    }
  }, []);
  useEffect(() => {
    authConfig().then(setCfg);
    if (auth.key() || document.cookie.includes('zanshin_console') || window.location.pathname.startsWith('/console')) void loadMe();
    else setMe(null);
  }, [loadMe]);
  const [theme, setTheme] = useState<'light' | 'dark'>(initialTheme);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem(THEME_KEY, theme);
  }, [theme]);
  useEffect(() => localStorage.setItem('zanshin.nav', collapsed ? 'collapsed' : 'open'), [collapsed]);
  useEffect(() => {
    const h = () => { auth.clear(); setMe(null); };
    window.addEventListener('zanshin:unauthorized', h);
    const f = (e: Event) => { const m = String((e as CustomEvent).detail ?? ''); const req = m.match(/permission required: ([a-z_]+:[a-z]+)/); toast(req ? `No access — needs ${req[1]}` : m || 'Forbidden', 'bad'); };
    window.addEventListener('zanshin:forbidden', f);
    const k = (e: KeyboardEvent) => { if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); setPalette((p) => !p); } };
    window.addEventListener('keydown', k);
    return () => { window.removeEventListener('zanshin:unauthorized', h); window.removeEventListener('zanshin:forbidden', f); window.removeEventListener('keydown', k); };
  }, []);
  if (me === undefined) return <div className="login"><div className="card">Loading…</div></div>;
  if (!me) return <Login cfg={cfg} onDone={() => { authConfig().then(setCfg); void loadMe(); }} />;
  const RES: Record<string, string> = { '/flags': 'flags', '/audiences': 'audiences', '/experiments': 'experiments', '/live-events': 'live_events', '/messages': 'messages', '/templates': 'templates', '/journeys': 'journeys', '/integrations': 'integrations', '/identities': 'identities', '/taxonomy': 'taxonomy', '/insights': 'analytics', '/analytics': 'analytics', '/metrics': 'metrics', '/webhooks': 'webhooks', '/data': 'data', '/api-keys': 'api_keys', '/users': 'users', '/audit': 'audit', '/settings': 'settings' };
  const groups = GROUPS.map((g) => ({ ...g, items: g.items.filter(([to]) => !RES[to] || allows(me.permissions, RES[to], 'read')) }));
  const pages: PaletteItem[] = groups.flatMap((g) => g.items.map(([to, label]) => ({ label, to, group: g.name || 'Pages' })));
  const signOut = async (e: React.MouseEvent) => {
    e.preventDefault();
    auth.clear();
    if (me.auth === 'oidc' || me.auth === 'user') await ssoLogout();
    setMe(null);
  };
  return (
    <MeContext.Provider value={me}>
      <div className={'shell' + (collapsed ? ' collapsed' : '')}>
        <nav className="side">
          <div className="brand" onClick={() => setCollapsed((c) => !c)} title="Toggle sidebar">
            <span className="logo">残</span><span className="brand-text">Zanshin<small>LiveOps console</small></span>
          </div>
          <button className="search-btn" onClick={() => setPalette(true)}><Icon d="M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM21 21l-5-5" /><span className="brand-text">Search…<kbd>⌘K</kbd></span></button>
          <div className="nav-scroll">
            {groups.map((g) => (
              <div key={g.name} className="nav-group">
                {g.name && <div className="nav-title">{g.name}</div>}
                {g.items.map(([to, label, d]) => <NavLink key={to} to={to} end={to === '/'} title={label}><Icon d={d} /><span className="brand-text">{label}</span></NavLink>)}
              </div>
            ))}
          </div>
          <div className="side-foot">
            <div className="whoami" title={me.auth}>
              <span className="avatar">{(me.name || '?').slice(0, 1).toUpperCase()}</span>
              <NavLink to="/account" className="brand-text trunc" title="Your account">{me.name}</NavLink>
              <span className={'role role-' + me.role + ' brand-text'}>{me.role}</span>
            </div>
            <button className="theme-btn" onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}>{theme === 'dark' ? 'Light theme' : 'Dark theme'}</button>
            <a href="#" onClick={signOut}>Sign out</a>
          </div>
        </nav>
        <div className="main-col">
          <header className="topbar"><Crumbs /><div className="spacer" />{me.read_only && <span className="badge warn">read-only</span>}{me.must_change_password && <NavLink to="/account" className="badge warn">change your password</NavLink>}</header>
          <main className="main">
            <Routes>
              <Route path="/" element={<Dashboard />} />
              <Route path="/flags/*" element={<Flags />} />
              <Route path="/experiments/*" element={<Experiments />} />
              <Route path="/audiences/*" element={<Audiences />} />
              <Route path="/live-events/*" element={<LiveEvents />} />
              <Route path="/messages/*" element={<Messages />} />
              <Route path="/identities/*" element={<Identities />} />
              <Route path="/analytics" element={<Analytics />} />
              <Route path="/taxonomy/*" element={<Taxonomy />} />
              <Route path="/metrics/*" element={<Metrics />} />
              <Route path="/templates/*" element={<Templates />} />
              <Route path="/integrations/*" element={<Integrations />} />
              <Route path="/webhooks/*" element={<Webhooks />} />
              <Route path="/journeys/*" element={<Journeys />} />
              <Route path="/insights/*" element={<Insights />} />
              <Route path="/data/*" element={<Data />} />
              {allows(me.permissions, 'api_keys', 'read') && <Route path="/api-keys" element={<ApiKeys />} />}
              {allows(me.permissions, 'users', 'read') && <Route path="/users" element={<Users />} />}
              <Route path="/account" element={<Account onChanged={() => void loadMe()} />} />
              <Route path="/audit" element={<Audit />} />
              <Route path="/settings/*" element={<Settings />} />
              <Route path="*" element={<Navigate to="/" replace />} />
            </Routes>
          </main>
        </div>
        <Palette pages={pages} open={palette} onClose={() => setPalette(false)} />
        <Toasts />
      </div>
    </MeContext.Provider>
  );
}
