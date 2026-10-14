import { useCallback, useEffect, useState } from 'react';
import { NavLink, Navigate, Route, Routes } from 'react-router-dom';
import { auth, authConfig, get, ssoLogout, type Me } from './api';
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
import ApiKeys from './pages/ApiKeys';
import Audit from './pages/Audit';

function Login({ onDone, sso }: { onDone: () => void; sso: boolean }) {
  const [key, setKey] = useState('');
  const [err, setErr] = useState<string | null>(() => new URLSearchParams(window.location.search).get('error'));
  const [busy, setBusy] = useState(false);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    auth.set(key.trim());
    try {
      await get('/me');
      onDone();
    } catch (e) {
      auth.clear();
      setErr((e as Error).message || 'Invalid key');
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="login">
      <form className="card" onSubmit={submit}>
        <div className="brand">Zanshin<small>admin console</small></div>
        {sso && (
          <>
            <a className="btn" href="/console/auth/login" style={{ width: '100%', display: 'block', textAlign: 'center', boxSizing: 'border-box' }}>Sign in with SSO</a>
            <div className="hint" style={{ textAlign: 'center', margin: '12px 0' }}>or use an admin API key</div>
          </>
        )}
        <label className="field">
          <span className="label">Admin API key</span>
          <input type="password" autoFocus={!sso} value={key} onChange={(e) => setKey(e.target.value)} placeholder="zk_a_…" />
          <span className="hint">The bootstrap key from config or an admin key minted via /admin/v1/api-keys. Stored in this browser only.</span>
        </label>
        {err && <div className="error">{err}</div>}
        <button className="btn" disabled={!key || busy} style={{ width: '100%' }}>Sign in</button>
      </form>
    </div>
  );
}

const NAV: [string, string][] = [
  ['/', 'Overview'],
  ['/flags', 'Feature flags'],
  ['/experiments', 'Experiments'],
  ['/audiences', 'Audiences'],
  ['/live-events', 'Live events'],
  ['/messages', 'Messages'],
  ['/identities', 'Identities'],
  ['/analytics', 'Analytics'],
  ['/taxonomy', 'Taxonomy'],
  ['/metrics', 'Metrics'],
  ['/templates', 'Templates'],
  ['/integrations', 'Integrations'],
  ['/api-keys', 'API keys'],
  ['/audit', 'Audit log'],
];

const THEME_KEY = 'zanshin.theme';
function initialTheme(): 'light' | 'dark' {
  const saved = localStorage.getItem(THEME_KEY);
  if (saved === 'light' || saved === 'dark') return saved;
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export default function App() {
  const [me, setMe] = useState<Me | null | undefined>(undefined);
  const [sso, setSso] = useState(false);
  const loadMe = useCallback(async () => {
    try {
      setMe(await get<Me>('/me'));
    } catch {
      setMe(null);
    }
  }, []);
  useEffect(() => {
    authConfig().then((c) => setSso(c.oidc));
    if (auth.key() || document.cookie.includes('zanshin_console=') || window.location.pathname.startsWith('/console')) void loadMe();
    else setMe(null);
  }, [loadMe]);
  const [theme, setTheme] = useState<'light' | 'dark'>(initialTheme);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem(THEME_KEY, theme);
  }, [theme]);
  useEffect(() => {
    const h = () => { auth.clear(); setMe(null); };
    window.addEventListener('zanshin:unauthorized', h);
    return () => window.removeEventListener('zanshin:unauthorized', h);
  }, []);
  if (me === undefined) return <div className="login"><div className="card">Loading…</div></div>;
  if (!me) return <Login sso={sso} onDone={loadMe} />;
  const nav = NAV.filter(([to]) => to !== '/api-keys' || me.role === 'admin');
  const signOut = async (e: React.MouseEvent) => {
    e.preventDefault();
    auth.clear();
    if (me.auth === 'oidc') await ssoLogout();
    setMe(null);
  };
  return (
    <div className="shell">
      <nav className="side">
        <div className="brand">Zanshin<small>admin console</small></div>
        {nav.map(([to, label]) => <NavLink key={to} to={to} end={to === '/'}>{label}</NavLink>)}
        <div className="spacer" />
        <div className="whoami" title={me.auth}>
          <span className="trunc">{me.name}</span>
          <span className={'role role-' + me.role}>{me.role}</span>
        </div>
        <button className="theme-btn" onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}>{theme === 'dark' ? 'Light theme' : 'Dark theme'}</button>
        <a href="#" onClick={signOut}>Sign out</a>
      </nav>
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
          {me.role === 'admin' && <Route path="/api-keys" element={<ApiKeys />} />}
          <Route path="/audit" element={<Audit />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}
