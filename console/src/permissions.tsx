import { useEffect, useState } from 'react';
import { get } from './api';

export type Action = 'read' | 'write' | 'delete';
export type Permissions = Record<string, Action[]>;
export type Archetype = { name: string; title: string; description: string; permissions: Permissions };
export type Catalog = { resources: string[]; actions: Action[]; archetypes: Archetype[] };

let cached: Catalog | null = null;
export function useCatalog(): Catalog | null {
  const [c, setC] = useState<Catalog | null>(cached);
  useEffect(() => {
    if (cached) return;
    get<Catalog>('/permissions').then((r) => { cached = r; setC(r); }).catch(() => undefined);
  }, []);
  return c;
}

/** Mirrors `Permissions::allows` on the server: a resource entry overrides `*`. */
export function allows(p: Permissions | undefined, resource: string, action: Action): boolean {
  if (!p) return true;
  const has = (set: Action[] | undefined) => !!set && (set.includes(action) || (action === 'read' && (set.includes('write') || set.includes('delete'))));
  return resource in p ? has(p[resource]) : has(p['*']);
}

export const RESOURCE_TITLES: Record<string, string> = {
  flags: 'Feature flags', audiences: 'Audiences', experiments: 'Experiments', metrics: 'Metrics', live_events: 'Live events',
  messages: 'Messages & push', templates: 'Templates', integrations: 'Integrations', webhooks: 'Webhooks', journeys: 'Journeys',
  identities: 'Identities', taxonomy: 'Taxonomy', analytics: 'Analytics & dashboards', data: 'Data & exports', audit: 'Audit log',
  api_keys: 'API keys', users: 'Console users', settings: 'Settings',
};

/** Editable resource × action grid. `*` row applies to everything not listed explicitly. */
export function PermissionMatrix({ value, onChange, resources, actions, disabled }: { value: Permissions; onChange: (p: Permissions) => void; resources: string[]; actions: Action[]; disabled?: boolean }) {
  const toggle = (res: string, act: Action) => {
    const next: Permissions = { ...value };
    const set = new Set(next[res] ?? []);
    if (set.has(act)) set.delete(act); else set.add(act);
    if (set.size) next[res] = actions.filter((a) => set.has(a)); else delete next[res];
    onChange(next);
  };
  const rows = ['*', ...resources];
  return (
    <table className="table compact perm-matrix">
      <thead><tr><th>Resource</th>{actions.map((a) => <th key={a} style={{ textAlign: 'center' }}>{a}</th>)}</tr></thead>
      <tbody>
        {rows.map((r) => (
          <tr key={r} className={r === '*' ? 'muted' : ''}>
            <td>{r === '*' ? <em>everything else (*)</em> : (RESOURCE_TITLES[r] ?? r)}</td>
            {actions.map((a) => {
              const explicit = !!value[r]?.includes(a);
              const inherited = r !== '*' && !(r in value) && allows(value, r, a);
              return (
                <td key={a} style={{ textAlign: 'center' }}>
                  <input type="checkbox" disabled={disabled} checked={explicit || inherited} onChange={() => toggle(r, a)} title={inherited ? 'inherited from *' : ''} style={inherited ? { opacity: 0.45 } : undefined} />
                </td>
              );
            })}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** Archetype picker + matrix; picking a preset replaces the matrix, editing the matrix clears the preset. */
export function PermissionEditor({ archetype, permissions, onChange, disabled }: { archetype: string | null; permissions: Permissions; onChange: (a: string | null, p: Permissions) => void; disabled?: boolean }) {
  const cat = useCatalog();
  if (!cat) return <div className="hint">Loading permission catalog…</div>;
  return (
    <div className="stack">
      <label className="field">
        <span className="label">Preset</span>
        <select value={archetype ?? ''} disabled={disabled} onChange={(e) => { const a = cat.archetypes.find((x) => x.name === e.target.value); onChange(a ? a.name : null, a ? a.permissions : permissions); }}>
          <option value="">custom</option>
          {cat.archetypes.map((a) => <option key={a.name} value={a.name}>{a.title} — {a.description}</option>)}
        </select>
      </label>
      <PermissionMatrix value={permissions} onChange={(p) => onChange(null, p)} resources={cat.resources} actions={cat.actions} disabled={disabled} />
    </div>
  );
}
