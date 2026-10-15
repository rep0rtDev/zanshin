import { useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { Route, Routes, useNavigate, useParams } from 'react-router-dom';
import { del, get, post, put, useLoad } from './api';
import { useCanEdit } from './me';
import { toast } from './toast';
import { Badge, Confirm, Empty, ErrorBox, Menu, Page, Pager, Table, fmtDate } from './ui';
import type { Col } from './ui';
import { locale, t } from './i18n';

export type Keyed = { key: string; version?: number; updated_at?: string; name?: string; description?: string; tags?: string[]; status?: string; enabled?: boolean };

export type Tab<T> = { id: string; label: ReactNode; render: (item: T, reload: () => void) => ReactNode };

type Props<T extends Keyed> = {
  title: string;
  path: string;
  blurb?: ReactNode;
  cols: Col<T>[];
  blank: () => Partial<T>;
  form: (draft: Partial<T>, set: (patch: Partial<T>) => void, isNew: boolean) => ReactNode;
  /** Legacy side panel; rendered as an "Overview" tab next to Settings. */
  extra?: (item: T, reload: () => void) => ReactNode;
  tabs?: Tab<T>[];
  toBody?: (draft: Partial<T>, isNew: boolean) => unknown;
  sort?: (a: T, b: T) => number;
  above?: ReactNode;
  /** Server-side copy endpoint (`POST {path}/{key}/copy { key }`), shown in the row menu when set. */
  copy?: boolean;
  statusOf?: (item: T) => string | undefined;
  /** Extra text searched by the list search box. */
  searchText?: (item: T) => string;
  historyKind?: string;
  /** Multi-step creation flow; the last implicit step is a review of the draft. */
  wizard?: { label: string; render: (draft: Partial<T>, set: (patch: Partial<T>) => void) => ReactNode }[];
};

const READONLY = ['id', 'version', 'created_at', 'updated_at', 'salt', 'status'];
const strip = (d: Record<string, unknown>) => Object.fromEntries(Object.entries(d).filter(([k, v]) => !READONLY.includes(k) && v !== undefined));
const kindOf = (path: string) => path.replace(/^\//, '').replace(/-/g, '_');
/** List endpoints answer either a bare array or `{ items: [...] }`. */
const asList = <T,>(d: unknown): T[] => (Array.isArray(d) ? d : ((d as { items?: T[] } | null)?.items ?? [])) as T[];
/** Detail endpoints answer either the entity or `{ <kind>: entity, ...extras }`. */
const asItem = <T extends Keyed>(d: unknown): T => {
  const o = d as Record<string, unknown>;
  if (o && typeof o === 'object' && !('key' in o)) {
    const inner = Object.values(o).find((v) => v && typeof v === 'object' && !Array.isArray(v) && 'key' in (v as object));
    if (inner) return inner as T;
  }
  return d as T;
};

type ListFilter = { search: string; tag: string | null; status: string | null; sortBy: 'key' | 'updated' };

export function EntityPage<T extends Keyed>(p: Props<T>) {
  return (
    <Routes>
      <Route index element={<List {...p} />} />
      <Route path="new" element={<Detail {...p} isNew />} />
      <Route path=":key/*" element={<Detail {...p} />} />
    </Routes>
  );
}

const defaultStatus = <T extends Keyed>(it: T) => it.status ?? (it.enabled === undefined ? undefined : it.enabled ? 'enabled' : 'disabled');

function List<T extends Keyed>({ title, path, blurb, cols, sort, above, copy, statusOf = defaultStatus, searchText }: Props<T>) {
  const nav = useNavigate();
  const canEdit = useCanEdit();
  const q = useLoad(() => get<unknown>(path).then(asList<T>));
  const filterKey = `zanshin.list.${path}`;
  const saved = useMemo<Partial<ListFilter>>(() => {
    try { return JSON.parse(localStorage.getItem(filterKey) || '{}'); } catch { return {}; }
  }, [filterKey]);
  const [search, setSearch] = useState(saved.search ?? '');
  const [tag, setTag] = useState<string | null>(saved.tag ?? null);
  const [status, setStatus] = useState<string | null>(saved.status ?? null);
  const [sortBy, setSortBy] = useState<'key' | 'updated'>(saved.sortBy ?? 'key');
  const [page, setPage] = useState(0);
  useEffect(() => {
    localStorage.setItem(filterKey, JSON.stringify({ search, tag, status, sortBy } satisfies ListFilter));
  }, [filterKey, search, tag, status, sortBy]);
  const [size, setSize] = useState(() => Number(localStorage.getItem('zanshin.pagesize')) || 25);
  useEffect(() => localStorage.setItem('zanshin.pagesize', String(size)), [size]);
  useEffect(() => setPage(0), [search, tag, status, size]);
  const all = q.data ?? [];
  const tags = useMemo(() => Array.from(new Set(all.flatMap((x) => x.tags ?? []))).sort(), [all]);
  const statuses = useMemo(() => Array.from(new Set(all.map(statusOf).filter((s): s is string => !!s))).sort(), [all, statusOf]);
  const rows = useMemo(() => {
    const s = search.trim().toLowerCase();
    const out = all.filter((x) =>
      (!tag || (x.tags ?? []).includes(tag)) && (!status || statusOf(x) === status) &&
      (!s || `${x.key} ${x.name ?? ''} ${x.description ?? ''} ${(x.tags ?? []).join(' ')} ${searchText?.(x) ?? ''}`.toLowerCase().includes(s)));
    out.sort(sortBy === 'updated' ? (a, b) => (b.updated_at ?? '').localeCompare(a.updated_at ?? '') : sort ?? ((a, b) => a.key.localeCompare(b.key)));
    return out;
  }, [all, search, tag, status, sortBy, sort, statusOf, searchText]);
  const pages = Math.max(1, Math.ceil(rows.length / size));
  const slice = rows.slice(page * size, page * size + size);
  const remove = async (r: T) => { try { await del(`${path}/${encodeURIComponent(r.key)}`); toast(`Deleted ${r.key}`); q.reload(); } catch (e) { toast((e as Error).message, 'bad'); } };
  const duplicate = async (r: T) => {
    const key = prompt(`New key for a copy of ${r.key}`, `${r.key}-copy`);
    if (!key) return;
    try {
      if (copy) await post(`${path}/${encodeURIComponent(r.key)}/copy`, { key });
      else { const src = asItem<T>(await get<unknown>(`${path}/${encodeURIComponent(r.key)}`)); await post(path, { ...strip(src as Record<string, unknown>), key, name: src.name ? `${src.name} (copy)` : undefined }); }
      toast(`Created ${key}`); nav(encodeURIComponent(key));
    } catch (e) { toast((e as Error).message, 'bad'); }
  };
  const menuCol: Col<T> = { h: '', w: '44px', c: (r) => canEdit ? <Menu items={[{ label: 'Open', onClick: () => nav(encodeURIComponent(r.key)) }, { label: 'Duplicate', onClick: () => duplicate(r) }, { label: 'Delete', danger: true, onClick: () => { if (confirm(`Delete ${r.key}?`)) void remove(r); } }]} /> : null };
  const updatedCol: Col<T> = { h: 'Updated', w: '150px', c: (r) => <span className="hint" style={{ whiteSpace: 'nowrap' }}>{r.updated_at ? fmtDate(r.updated_at) : ''}</span> };
  const showUpdated = all.some((x) => x.updated_at) && !cols.some((c) => c.h === 'Updated');
  const filtered = !!(search || tag || status);
  return (
    <Page title={<span>{title}{q.data && <span className="count">{rows.length}{filtered ? ` / ${all.length}` : ''}</span>}</span>} actions={canEdit && <button className="btn" onClick={() => nav('new')}>{locale === 'en' ? `New ${title.replace(/s$/, '').toLowerCase()}` : t('Create')}</button>}>
      {blurb && <p className="muted" style={{ marginTop: -8 }}>{blurb}</p>}
      {above}
      <ErrorBox error={q.error} />
      {(all.length > 0 || filtered) && (
        <div className="toolbar">
          <input className="search" placeholder={locale === 'en' ? `Search ${title.toLowerCase()}…` : t('Search…')} value={search} onChange={(e) => setSearch(e.target.value)} />
          {statuses.length > 1 && <div className="chips">{statuses.map((s) => <button key={s} className={'chip' + (status === s ? ' on' : '')} onClick={() => setStatus(status === s ? null : s)}>{s}</button>)}</div>}
          {tags.length > 0 && <div className="chips">{tags.slice(0, 12).map((t) => <button key={t} className={'chip' + (tag === t ? ' on' : '')} onClick={() => setTag(tag === t ? null : t)}>#{t}</button>)}</div>}
          <div className="spacer" />
          <select value={sortBy} onChange={(e) => setSortBy(e.target.value as 'key' | 'updated')} style={{ width: 'auto' }}><option value="key">{t('Sort: key')}</option><option value="updated">{t('Sort: recently updated')}</option></select>
        </div>
      )}
      {q.data && all.length === 0 ? (
        <Empty title={`No ${title.toLowerCase()} yet`} hint={blurb ? undefined : 'Create the first one to get started.'} action={canEdit && <button className="btn" onClick={() => nav('new')}>Create {title.replace(/s$/, '').toLowerCase()}</button>} />
      ) : q.data && rows.length === 0 ? (
        <Empty title={t('Nothing matches')} action={<button className="btn ghost" onClick={() => { setSearch(''); setTag(null); setStatus(null); }}>{t('Clear filters')}</button>} />
      ) : (
        <Table<T> cols={showUpdated ? [...cols, updatedCol, menuCol] : [...cols, menuCol]} rows={q.data ? slice : null} rowKey={(r) => r.key} onRow={(r) => nav(encodeURIComponent(r.key))} />
      )}
      <Pager page={page} pages={pages} size={size} onPage={setPage} onSize={setSize} total={rows.length} />
    </Page>
  );
}

type Snapshot = { id: string; version: number; deleted: boolean; actor: string; at: string; body: unknown };
type AuditEntry = { id: string; at: string; actor: string; method: string; path: string; status: number };

function HistoryTab({ kind, k, reload }: { kind: string; k: string; reload: () => void }) {
  const q = useLoad(() => get<{ items: Snapshot[] }>(`/config/history?kind=${encodeURIComponent(kind)}&key=${encodeURIComponent(k)}&limit=100`), [kind, k]);
  const canEdit = useCanEdit();
  const [open, setOpen] = useState<string | null>(null);
  const revert = (id: string) => post(`/config/history/${id}/revert`, {}).then(() => { toast(t('Reverted')); q.reload(); reload(); }, (e: Error) => toast(e.message, 'bad'));
  return (
    <>
      <ErrorBox error={q.error} />
      <Table<Snapshot> rows={q.data?.items} rowKey={(s) => s.id} empty={t('No recorded changes yet')} cols={[
        { h: 'When', c: (s) => fmtDate(s.at) }, { h: 'Change', c: (s) => s.deleted ? <Badge tone="bad">deleted</Badge> : <span>v{s.version}</span> }, { h: 'By', c: (s) => <span className="hint">{s.actor}</span> },
        { h: '', c: (s) => <span className="inline"><button className="btn ghost sm" onClick={() => setOpen(open === s.id ? null : s.id)}>{open === s.id ? 'Hide' : 'Show'}</button>{canEdit && <button className="btn ghost sm" onClick={() => revert(s.id)}>{t('Revert')}</button>}</span> },
      ]} />
      {open && <pre className="code" style={{ marginTop: 10, maxHeight: 360 }}>{JSON.stringify(q.data?.items.find((s) => s.id === open)?.body, null, 2)}</pre>}
    </>
  );
}

function ActivityTab({ path, k }: { path: string; k: string }) {
  const q = useLoad(() => get<{ items: AuditEntry[] }>(`/audit?path=${encodeURIComponent(`/admin/v1${path}/${encodeURIComponent(k)}`)}&limit=100`), [path, k]);
  return (
    <>
      <ErrorBox error={q.error} />
      <Table<AuditEntry> rows={q.data?.items} rowKey={(e) => e.id} empty={t('No admin activity recorded')} cols={[
        { h: 'When', c: (e) => fmtDate(e.at) }, { h: 'Actor', c: (e) => e.actor }, { h: 'Action', c: (e) => <span className="mono">{e.method} {e.path.replace('/admin/v1', '')}</span> },
        { h: 'Status', c: (e) => <Badge tone={e.status < 300 ? 'ok' : e.status < 500 ? 'warn' : 'bad'}>{e.status}</Badge> },
      ]} />
    </>
  );
}

function Detail<T extends Keyed>({ title, path, blank, form, extra, tabs = [], toBody, isNew, statusOf = defaultStatus, historyKind, wizard }: Props<T> & { isNew?: boolean }) {
  const [step, setStep] = useState(0);
  const { key } = useParams();
  const nav = useNavigate();
  const canEdit = useCanEdit();
  const [draft, setDraft] = useState<Partial<T> | null>(isNew ? blank() : null);
  const [item, setItem] = useState<T | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [tick, setTick] = useState(0);
  const [tab, setTab] = useState('settings');
  const reload = () => setTick((t) => t + 1);
  useEffect(() => {
    if (isNew || !key) return;
    get<unknown>(`${path}/${encodeURIComponent(key)}`).then(asItem<T>).then((it) => { setItem(it); setDraft(it); setErr(null); setDirty(false); }, setErr);
  }, [path, key, isNew, tick]);
  useEffect(() => {
    if (!dirty) return;
    const h = (e: BeforeUnloadEvent) => { e.preventDefault(); };
    window.addEventListener('beforeunload', h);
    return () => window.removeEventListener('beforeunload', h);
  }, [dirty]);
  const set = (patch: Partial<T>) => { setDraft((d) => ({ ...(d ?? {}), ...patch })); setDirty(true); };
  const save = async () => {
    if (!draft || busy) return;
    setBusy(true);
    try {
      const body = toBody ? toBody(draft, !!isNew) : strip(draft as Record<string, unknown>);
      const r = isNew ? await post<T>(path, body) : await put<T>(`${path}/${encodeURIComponent(key!)}`, body);
      setErr(null); setDirty(false);
      toast(isNew ? `Created ${r.key ?? draft.key}` : 'Saved');
      if (isNew) nav(`../${encodeURIComponent(r.key ?? (draft.key as string))}`, { replace: true, relative: 'path' });
      else { setItem(r); setDraft(r); }
    } catch (e) { setErr(e as Error); } finally { setBusy(false); }
  };
  useEffect(() => {
    const h = (e: KeyboardEvent) => { if ((e.metaKey || e.ctrlKey) && e.key === 's') { e.preventDefault(); void save(); } };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  });
  const remove = async () => {
    try { await del(`${path}/${encodeURIComponent(key!)}`); toast(`Deleted ${key}`); nav('..', { relative: 'path' }); } catch (e) { setErr(e as Error); }
  };
  const back = () => { if (!dirty || confirm('Discard unsaved changes?')) nav(isNew ? '..' : '..', { relative: 'path' }); };
  const allTabs: Tab<T>[] = [
    { id: 'settings', label: 'Settings', render: () => <div className="card">{form(draft!, set, !!isNew)}</div> },
    ...(extra ? [{ id: 'overview', label: 'Overview', render: extra } as Tab<T>] : []),
    ...tabs,
    ...(!isNew ? [
      { id: 'history', label: 'History', render: (it: T, r: () => void) => <HistoryTab kind={historyKind ?? kindOf(path)} k={it.key} reload={r} /> },
      { id: 'activity', label: 'Activity', render: (it: T) => <ActivityTab path={path} k={it.key} /> },
    ] : []),
  ];
  const st = item ? statusOf(item) : undefined;
  const current = allTabs.find((t) => t.id === tab) ?? allTabs[0];
  return (
    <Page
      title={<span className="inline"><a href="#" className="muted" onClick={(e) => { e.preventDefault(); back(); }}>{title}</a><span className="muted">/</span><span>{isNew ? 'new' : key}</span>{st && <Badge tone={/running|enabled|active|live|ok/.test(st) ? 'ok' : /paused|draft|pending/.test(st) ? 'warn' : /stopped|failed|disabled/.test(st) ? 'muted' : 'info'}>{st}</Badge>}{(item?.tags ?? []).map((t) => <span key={t} className="chip" style={{ cursor: 'default' }}>#{t}</span>)}</span>}
      actions={
        <>
          {dirty && <span className="hint">{t('Unsaved changes')}</span>}
          {!isNew && canEdit && <Confirm danger onConfirm={remove}>{t('Delete')}</Confirm>}
          {canEdit && !(isNew && wizard) && <button className="btn" disabled={busy || !draft || (!isNew && !dirty)} onClick={save} title="⌘S">{isNew ? 'Create' : 'Save'}</button>}
        </>
      }
    >
      <ErrorBox error={err} />
      {!draft ? <div className="muted">{t('Loading…')}</div> : (
        <>
          {(item || isNew) && allTabs.length > 1 && !isNew && (
            <div className="tabs">{allTabs.map((t) => <button key={t.id} className={t.id === current.id ? 'active' : ''} onClick={() => setTab(t.id)}>{t.label}</button>)}</div>
          )}
          <fieldset disabled={!canEdit} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
            {isNew && wizard ? (
              <div className="wizard">
                <ol className="steps">{[...wizard.map((w) => w.label), t('Review')].map((l, i) => <li key={l} className={i === step ? 'on' : i < step ? 'done' : ''} onClick={() => i < step && setStep(i)}>{l}</li>)}</ol>
                <div className="card">
                  {step < wizard.length ? wizard[step].render(draft, set) : <><p className="hint">{t('Everything below will be created as') + ' '}<strong>{draft.key || '(no key)'}</strong>.</p><pre className="code" style={{ maxHeight: 360 }}>{JSON.stringify(toBody ? toBody(draft, true) : strip(draft as Record<string, unknown>), null, 2)}</pre></>}
                  <div className="inline" style={{ marginTop: 14, justifyContent: 'space-between', width: '100%' }}>
                    <button className="btn ghost" disabled={step === 0} onClick={() => setStep(step - 1)}>{t('Back')}</button>
                    {step < wizard.length ? <button className="btn" disabled={step === 0 && !draft.key} onClick={() => setStep(step + 1)}>{t('Next')}</button> : <button className="btn" disabled={busy} onClick={save}>{t('Create')}</button>}
                  </div>
                </div>
              </div>
            ) : isNew || current.id === 'settings' ? <div className="card">{form(draft, set, !!isNew)}</div> : item && current.render(item, reload)}
          </fieldset>
        </>
      )}
      {item && <div className="hint">version {item.version} · updated {item.updated_at ? fmtDate(item.updated_at) : ''}</div>}
    </Page>
  );
}

export const Text = ({ label, value, onChange, hint, placeholder, mono, disabled }: { label: ReactNode; value: string | undefined; onChange: (v: string) => void; hint?: ReactNode; placeholder?: string; mono?: boolean; disabled?: boolean }) => (
  <label className="field">
    <span className="label">{label}</span>
    <input className={mono ? 'mono' : ''} value={value ?? ''} onChange={(e) => onChange(e.target.value)} placeholder={placeholder} disabled={disabled} />
    {hint && <span className="hint">{hint}</span>}
  </label>
);

export const Num = ({ label, value, onChange, hint, min, max, step }: { label: ReactNode; value: number | null | undefined; onChange: (v: number | undefined) => void; hint?: ReactNode; min?: number; max?: number; step?: number }) => (
  <label className="field">
    <span className="label">{label}</span>
    <input type="number" value={value ?? ''} min={min} max={max} step={step} onChange={(e) => onChange(e.target.value === '' ? undefined : Number(e.target.value))} />
    {hint && <span className="hint">{hint}</span>}
  </label>
);
export const DateTime = ({ label, value, onChange, hint }: { label: ReactNode; value: string | null | undefined; onChange: (v: string | null) => void; hint?: ReactNode }) => {
  const local = value ? new Date(value) : null;
  const pad = (n: number) => String(n).padStart(2, '0');
  const v = local ? `${local.getFullYear()}-${pad(local.getMonth() + 1)}-${pad(local.getDate())}T${pad(local.getHours())}:${pad(local.getMinutes())}` : '';
  return (
    <label className="field">
      <span className="label">{label}</span>
      <input type="datetime-local" value={v} onChange={(e) => onChange(e.target.value ? new Date(e.target.value).toISOString() : null)} />
      {hint && <span className="hint">{hint}</span>}
    </label>
  );
};

export const Tags = ({ value, onChange }: { value: string[] | undefined; onChange: (v: string[]) => void }) => (
  <Text label={t('Tags')} value={(value ?? []).join(', ')} onChange={(v) => onChange(v.split(',').map((s) => s.trim()).filter(Boolean))} hint="comma-separated" />
);

export const CONDITION_HINT = (
  <>
    Rule language: <code>{'{"all":[…]}'}</code>, <code>{'{"any":[…]}'}</code>, <code>{'{"not":{…}}'}</code> or a comparison{' '}
    <code>{'{"prop":"country","op":"in","value":["DE","FR"]}'}</code>. Ops: eq neq gt gte lt lte in not_in contains not_contains starts_with ends_with matches exists not_exists semver_eq semver_gt semver_gte semver_lt semver_lte.
    Props: <code>default.*</code>, <code>custom.*</code>, <code>computed.*</code>. <code>{'{"all":[]}'}</code> matches everyone; leave the field empty for no condition where it is optional.
  </>
);
