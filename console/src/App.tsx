import { useEffect, useState } from 'react';
import { NavLink, Navigate, Route, Routes } from 'react-router-dom';
import { auth, get } from './api';
import Dashboard from './pages/Dashboard';
import Flags from './pages/Flags';
import Experiments from './pages/Experiments';
import Audiences from './pages/Audiences';
import LiveEvents from './pages/LiveEvents';
import Messages from './pages/Messages';
import Identities from './pages/Identities';
import Analytics from './pages/Analytics';
import ApiKeys from './pages/ApiKeys';

function Login({ onDone }: { onDone: () => void }) {
  const [key, setKey] = useState('');
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    auth.set(key.trim());
    try {
      await get('/stats');
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
        <label className="field">
          <span className="label">Admin API key</span>
          <input type="password" autoFocus value={key} onChange={(e) => setKey(e.target.value)} placeholder="zk_a_…" />
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
  ['/api-keys', 'API keys'],
];

export default function App() {
  const [authed, setAuthed] = useState(!!auth.key());
  useEffect(() => {
    const h = () => { auth.clear(); setAuthed(false); };
    window.addEventListener('zanshin:unauthorized', h);
    return () => window.removeEventListener('zanshin:unauthorized', h);
  }, []);
  if (!authed) return <Login onDone={() => setAuthed(true)} />;
  return (
    <div className="shell">
      <nav className="side">
        <div className="brand">Zanshin<small>admin console</small></div>
        {NAV.map(([to, label]) => <NavLink key={to} to={to} end={to === '/'}>{label}</NavLink>)}
        <div className="spacer" />
        <a href="#" onClick={(e) => { e.preventDefault(); auth.clear(); setAuthed(false); }}>Sign out</a>
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
          <Route path="/api-keys" element={<ApiKeys />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}
