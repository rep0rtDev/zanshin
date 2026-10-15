import { useState } from 'react';
import { NavLink, Route, Routes } from 'react-router-dom';
import { get, post, project, put, useLoad } from '../api';
import { useMe } from '../me';
import { toast } from '../toast';
import { Badge, ErrorBox, Field, Page, displayTimezone, fmtDate, fmtNum, setDisplayTz } from '../ui';

type System = { version: string; started_at: string; uptime_secs: number; stores: { postgres: boolean; redis: boolean; clickhouse: boolean; event_store: string }; push: { provider: string }; auth: { oidc: boolean; saml?: boolean; scim?: boolean; access_ttl_secs: number; refresh_ttl_secs: number }; limits: Record<string, number>; taxonomy: { mode: string; auto_discover: boolean; geoip: boolean }; audit_retention_days: number; counts: Record<string, number> };
type TaxSettings = { mode: string; auto_discover: boolean; geoip_enabled: boolean; session_inactivity_secs: number };

function General() {
  const q = useLoad(() => get<System>('/system'), []);
  const me = useMe();
  const s = q.data;
  return (
    <div className="stack">
      <ErrorBox error={q.error} />
      {s && (
        <>
          <div className="grid c4">
            <div className="stat"><div className="stat-v">{s.version}</div><div className="stat-l">server version</div></div>
            <div className="stat"><div className="stat-v">{Math.floor(s.uptime_secs / 3600)}h {Math.floor((s.uptime_secs % 3600) / 60)}m</div><div className="stat-l">uptime · since {fmtDate(s.started_at)}</div></div>
            <div className="stat"><div className="stat-v">{s.stores.event_store}</div><div className="stat-l">event store</div></div>
            <div className="stat"><div className="stat-v">{s.push.provider}</div><div className="stat-l">push provider</div></div>
          </div>
          <div className="card">
            <strong>Stores</strong>
            <dl className="kv" style={{ marginTop: 8 }}>
              <dt>PostgreSQL</dt><dd><Badge tone={s.stores.postgres ? 'ok' : 'bad'}>{s.stores.postgres ? 'connected' : 'down'}</Badge></dd>
              <dt>Redis</dt><dd><Badge tone={s.stores.redis ? 'ok' : 'bad'}>{s.stores.redis ? 'connected' : 'down'}</Badge></dd>
              <dt>ClickHouse</dt><dd><Badge tone={s.stores.clickhouse ? 'ok' : 'muted'}>{s.stores.clickhouse ? 'enabled' : 'not configured (events in PostgreSQL)'}</Badge></dd>
            </dl>
          </div>
          <div className="card">
            <strong>Authentication</strong>
            <dl className="kv" style={{ marginTop: 8 }}>
              <dt>You</dt><dd>{me?.name} · <span className={'role role-' + me?.role}>{me?.role}</span> · via {me?.auth}</dd>
              <dt>Console SSO</dt><dd>{s.auth.oidc && <Badge tone="ok">OpenID Connect</Badge>} {s.auth.saml && <Badge tone="ok">SAML 2.0</Badge>} {s.auth.scim && <Badge tone="info">SCIM provisioning</Badge>} {!s.auth.oidc && !s.auth.saml && <Badge>password / admin API keys</Badge>}</dd>
              <dt>Player access token</dt><dd>{fmtNum(s.auth.access_ttl_secs)} s</dd>
              <dt>Player refresh token</dt><dd>{fmtNum(s.auth.refresh_ttl_secs)} s</dd>
              <dt>Audit retention</dt><dd>{s.audit_retention_days} days</dd>
            </dl>
          </div>
          <ConsolePrefs />
          <div className="card">
            <strong>Rate limits</strong>
            <p className="hint">Requests per minute per caller, by scope. Change them in the server config (<code>limits.*</code>).</p>
            <dl className="kv">{Object.entries(s.limits).map(([k, v]) => <><dt key={k + 'k'}>{k}</dt><dd key={k + 'v'}>{k === 'enabled' ? (v ? 'yes' : 'no') : v === 0 ? 'unlimited' : fmtNum(v)}</dd></>)}</dl>
          </div>
          <div className="card">
            <strong>Inventory</strong>
            <div className="grid c4" style={{ marginTop: 8 }}>{Object.entries(s.counts).map(([k, v]) => <div key={k}><div style={{ fontSize: 20, fontWeight: 650 }}>{fmtNum(v)}</div><div className="hint">{k.replace(/_/g, ' ')}</div></div>)}</div>
          </div>
        </>
      )}
    </div>
  );
}

type ConsoleSettings = { timezone: string; default_range_days: number | null };

const TZ_CHOICES = ['UTC', 'Europe/London', 'Europe/Berlin', 'Europe/Moscow', 'Asia/Dubai', 'Asia/Kolkata', 'Asia/Singapore', 'Asia/Tokyo', 'Australia/Sydney', 'America/Sao_Paulo', 'America/New_York', 'America/Chicago', 'America/Los_Angeles'];

/** Console-wide display preferences shared by every user (stored server-side). */
function ConsolePrefs() {
  const q = useLoad(() => get<ConsoleSettings>('/console/settings'), []);
  const [d, setD] = useState<ConsoleSettings | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const cur = d ?? q.data;
  const save = () => cur && put<ConsoleSettings>('/console/settings', cur).then((r) => { setDisplayTz(r.timezone); toast('Console settings saved'); setD(null); q.reload(); }, setErr);
  if (!cur) return <ErrorBox error={q.error} />;
  const tzs = TZ_CHOICES.includes(cur.timezone) || !cur.timezone ? TZ_CHOICES : [cur.timezone, ...TZ_CHOICES];
  return (
    <div className="card">
      <strong>Console</strong>
      <ErrorBox error={err} />
      <div className="row" style={{ alignItems: 'end', marginTop: 8 }}>
        <Field label="Display timezone" hint={`Applies to every date shown in the console, for all users. Currently ${displayTimezone()}.`}>
          <select value={cur.timezone} onChange={(e) => setD({ ...cur, timezone: e.target.value })}>
            <option value="">Browser default</option>
            {tzs.map((t) => <option key={t} value={t}>{t}</option>)}
          </select>
        </Field>
        <Field label="Or type an IANA name"><input value={cur.timezone} onChange={(e) => setD({ ...cur, timezone: e.target.value })} placeholder="Europe/Berlin" style={{ width: 180 }} /></Field>
        <Field label="Default analytics range (days)"><input type="number" min={1} max={365} value={cur.default_range_days ?? ''} onChange={(e) => setD({ ...cur, default_range_days: e.target.value ? Number(e.target.value) : null })} style={{ width: 90 }} /></Field>
        <button className="btn" onClick={save} disabled={!d}>Save</button>
      </div>
    </div>
  );
}

/** Effective server configuration with secrets masked, exportable as JSON. */
function Configuration() {
  const q = useLoad(() => get<{ config: Record<string, unknown>; sources: string[] }>('/system/config'), []);
  const [filter, setFilter] = useState('');
  const download = () => {
    if (!q.data) return;
    const blob = new Blob([JSON.stringify(q.data.config, null, 2)], { type: 'application/json' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = `zanshin-config-${project.get()}.json`;
    a.click();
    URL.revokeObjectURL(a.href);
  };
  const flat: [string, unknown][] = [];
  const walk = (v: unknown, path: string) => {
    if (v && typeof v === 'object' && !Array.isArray(v)) Object.entries(v as Record<string, unknown>).forEach(([k, x]) => walk(x, path ? `${path}.${k}` : k));
    else flat.push([path, v]);
  };
  if (q.data) walk(q.data.config, '');
  const rows = flat.filter(([k]) => !filter || k.includes(filter));
  return (
    <div className="stack">
      <ErrorBox error={q.error} />
      <div className="card">
        <div className="row" style={{ alignItems: 'center' }}>
          <strong>Effective configuration</strong>
          <span className="hint">defaults → <code>zanshin.yml</code> → <code>ZANSHIN__*</code> environment; secrets are masked as <code>***</code>. Read-only here — change it in the file / environment and restart.</span>
          <div className="spacer" />
          <input placeholder="filter keys" value={filter} onChange={(e) => setFilter(e.target.value)} style={{ maxWidth: 220 }} />
          <button className="btn ghost" onClick={download} disabled={!q.data}>Export JSON</button>
        </div>
        {q.data && (
          <table className="table" style={{ marginTop: 10 }}>
            <thead><tr><th style={{ width: 320 }}>Key</th><th>Value</th></tr></thead>
            <tbody>{rows.map(([k, v]) => <tr key={k}><td className="mono">{k}</td><td className="mono hint" style={{ wordBreak: 'break-all' }}>{v === null || v === undefined ? '—' : typeof v === 'string' ? v : JSON.stringify(v)}</td></tr>)}</tbody>
          </table>
        )}
      </div>
    </div>
  );
}

function Players() {
  const q = useLoad(() => get<TaxSettings>('/taxonomy/settings'), []);
  const [d, setD] = useState<TaxSettings | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const cur = d ?? q.data;
  const save = () => cur && put('/taxonomy/settings', cur).then(() => { toast('Settings saved'); q.reload(); }, setErr);
  if (!cur) return <ErrorBox error={q.error} />;
  return (
    <div className="card" style={{ maxWidth: 640 }}>
      <ErrorBox error={err ?? q.error} />
      <Field label="Event validation" hint="off: accept anything · warn: accept but record rejections · strict: reject events that fail their definition">
        <select value={cur.mode} onChange={(e) => setD({ ...cur, mode: e.target.value })}><option value="off">off</option><option value="warn">warn</option><option value="strict">strict</option></select>
      </Field>
      <Field label="Auto-discover event definitions"><input type="checkbox" checked={cur.auto_discover} onChange={(e) => setD({ ...cur, auto_discover: e.target.checked })} /></Field>
      <Field label="Session inactivity timeout (seconds)" hint="A play session ends after this long without events."><input type="number" min={30} value={cur.session_inactivity_secs} onChange={(e) => setD({ ...cur, session_inactivity_secs: Number(e.target.value) })} /></Field>
      <Field label="Geo-IP"><span className="hint">{cur.geoip_enabled ? 'enabled — country / region defaults are filled from the client IP' : 'not configured (taxonomy.geoip_db in server config)'}</span></Field>
      <button className="btn" onClick={save} disabled={!d}>Save</button>
    </div>
  );
}

type ProjectRow = { slug: string; name: string; description: string; archived: boolean; running: boolean; created_at: string };

function Projects() {
  const q = useLoad(() => get<{ items: ProjectRow[]; current: string }>('/projects'), []);
  const [slug, setSlug] = useState('');
  const [name, setName] = useState('');
  const [err, setErr] = useState<Error | null>(null);
  const cur = project.get();
  const create = () =>
    post('/projects', { slug, name }).then(() => { toast(`Project ${slug} created`); setSlug(''); setName(''); q.reload(); }, setErr);
  const archive = (p: ProjectRow) =>
    put(`/projects/${p.slug}`, { archived: !p.archived }).then(() => { toast(p.archived ? 'Project resumed' : 'Project archived'); q.reload(); }, setErr);
  if (cur !== 'default') {
    return <div className="card" style={{ maxWidth: 640 }}><p className="hint">Projects are managed from the <b>default</b> project — switch to it in the top bar.</p></div>;
  }
  return (
    <div className="card" style={{ maxWidth: 760 }}>
      <ErrorBox error={err ?? q.error} />
      <p className="hint">Each project is an isolated game / environment inside this deployment: its own configuration, players, events, keys and analytics (own PostgreSQL schema, Redis namespace and ClickHouse database). Console users and SSO are shared. SDKs and `zanshinctl` address a project with the <code>x-zanshin-project</code> header; session tokens carry it automatically.</p>
      <table className="table">
        <thead><tr><th>Slug</th><th>Name</th><th>Status</th><th>Created</th><th /></tr></thead>
        <tbody>
          {(q.data?.items ?? []).map((p) => (
            <tr key={p.slug}>
              <td><code>{p.slug}</code>{p.slug === cur && <Badge>current</Badge>}</td>
              <td>{p.name}</td>
              <td>{p.archived ? <Badge>archived</Badge> : p.running ? <Badge>running</Badge> : <Badge>stopped</Badge>}</td>
              <td>{fmtDate(p.created_at)}</td>
              <td>{p.slug !== 'default' && <button className="btn small" onClick={() => archive(p)}>{p.archived ? 'Resume' : 'Archive'}</button>}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <h3>New project</h3>
      <Field label="Slug" hint="2–32 chars, [a-z0-9_], starts with a letter; becomes the schema / database suffix"><input value={slug} onChange={(e) => setSlug(e.target.value)} placeholder="staging" /></Field>
      <Field label="Name"><input value={name} onChange={(e) => setName(e.target.value)} placeholder="Staging" /></Field>
      <button className="btn" onClick={create} disabled={!slug}>Create project</button>
    </div>
  );
}

export default function Settings() {
  const me = useMe();
  return (
    <Page title="Settings">
      <div className="tabs">
        <NavLink to="" end>General</NavLink>
        <NavLink to="players">Players & events</NavLink>
        <NavLink to="projects">Projects</NavLink>
        <NavLink to="configuration">Configuration</NavLink>
        <NavLink to="/taxonomy">Taxonomy →</NavLink>
        {me?.role === 'admin' && <NavLink to="/api-keys">API keys →</NavLink>}
        <NavLink to="/data">Data & backups →</NavLink>
      </div>
      <Routes>
        <Route index element={<General />} />
        <Route path="players" element={<Players />} />
        <Route path="projects" element={<Projects />} />
        <Route path="configuration" element={<Configuration />} />
      </Routes>
    </Page>
  );
}
