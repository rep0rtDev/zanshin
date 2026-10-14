import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { Route, Routes, useNavigate, useParams } from 'react-router-dom';
import { del, get, post, put, useLoad } from './api';
import { Confirm, ErrorBox, Page, Table } from './ui';
import type { Col } from './ui';

export type Keyed = { key: string; version?: number; updated_at?: string };

type Props<T extends Keyed> = {
  title: string;
  path: string;
  blurb?: ReactNode;
  cols: Col<T>[];
  blank: () => Partial<T>;
  form: (draft: Partial<T>, set: (patch: Partial<T>) => void, isNew: boolean) => ReactNode;
  extra?: (item: T, reload: () => void) => ReactNode;
  toBody?: (draft: Partial<T>, isNew: boolean) => unknown;
  sort?: (a: T, b: T) => number;
};

const READONLY = ['id', 'version', 'created_at', 'updated_at', 'salt', 'status'];
const strip = (d: Record<string, unknown>) => Object.fromEntries(Object.entries(d).filter(([k, v]) => !READONLY.includes(k) && v !== undefined));

export function EntityPage<T extends Keyed>(p: Props<T>) {
  return (
    <Routes>
      <Route index element={<List {...p} />} />
      <Route path="new" element={<Detail {...p} isNew />} />
      <Route path=":key" element={<Detail {...p} />} />
    </Routes>
  );
}

function List<T extends Keyed>({ title, path, blurb, cols, sort }: Props<T>) {
  const nav = useNavigate();
  const q = useLoad(() => get<T[]>(path));
  const rows = q.data ? [...q.data].sort(sort ?? ((a, b) => a.key.localeCompare(b.key))) : null;
  return (
    <Page title={title} actions={<button className="btn" onClick={() => nav('new')}>New</button>}>
      {blurb && <p className="muted" style={{ marginTop: -8 }}>{blurb}</p>}
      <ErrorBox error={q.error} />
      <Table<T> cols={cols} rows={rows} rowKey={(r) => r.key} onRow={(r) => nav(encodeURIComponent(r.key))} />
    </Page>
  );
}

function Detail<T extends Keyed>({ title, path, blank, form, extra, toBody, isNew }: Props<T> & { isNew?: boolean }) {
  const { key } = useParams();
  const nav = useNavigate();
  const [draft, setDraft] = useState<Partial<T> | null>(isNew ? blank() : null);
  const [item, setItem] = useState<T | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (isNew || !key) return;
    get<T>(`${path}/${encodeURIComponent(key)}`).then((it) => { setItem(it); setDraft(it); setErr(null); }, setErr);
  }, [path, key, isNew, tick]);
  const set = (patch: Partial<T>) => { setDraft((d) => ({ ...(d ?? {}), ...patch })); setSaved(false); };
  const save = async () => {
    if (!draft) return;
    setBusy(true);
    try {
      const body = toBody ? toBody(draft, !!isNew) : strip(draft as Record<string, unknown>);
      const r = isNew ? await post<T>(path, body) : await put<T>(`${path}/${encodeURIComponent(key!)}`, body);
      setErr(null);
      setSaved(true);
      if (isNew) nav(`../${encodeURIComponent(r.key ?? (draft.key as string))}`, { replace: true, relative: 'path' });
      else { setItem(r); setDraft(r); }
    } catch (e) { setErr(e as Error); } finally { setBusy(false); }
  };
  const remove = async () => {
    try { await del(`${path}/${encodeURIComponent(key!)}`); nav('..', { relative: 'path' }); } catch (e) { setErr(e as Error); }
  };
  return (
    <Page
      title={<span><a href="#" className="muted" onClick={(e) => { e.preventDefault(); nav(isNew ? '..' : '..', { relative: 'path' }); }}>{title}</a> / {isNew ? 'new' : key}</span>}
      actions={
        <>
          {saved && <span className="muted">Saved</span>}
          {!isNew && <Confirm danger onConfirm={remove}>Delete</Confirm>}
          <button className="btn" disabled={busy || !draft} onClick={save}>{isNew ? 'Create' : 'Save'}</button>
        </>
      }
    >
      <ErrorBox error={err} />
      {!draft ? <div className="muted">Loading…</div> : (
        <div className="grid" style={{ gridTemplateColumns: extra && item ? '1fr 1fr' : '1fr' }}>
          <div className="card">{form(draft, set, !!isNew)}</div>
          {extra && item && <div>{extra(item, () => setTick((t) => t + 1))}</div>}
        </div>
      )}
      {item && <div className="hint">version {item.version} · updated {item.updated_at ? new Date(item.updated_at).toLocaleString() : ''}</div>}
    </Page>
  );
}

export const Text = ({ label, value, onChange, hint, placeholder, mono, disabled }: { label: ReactNode; value: string | undefined; onChange: (v: string) => void; hint?: ReactNode; placeholder?: string; mono?: boolean; disabled?: boolean }) => (
  <label className="field">
    <span className="label">{label}</span>
    <input className={mono ? 'mono' : undefined} value={value ?? ''} onChange={(e) => onChange(e.target.value)} placeholder={placeholder} disabled={disabled} />
    {hint && <span className="hint">{hint}</span>}
  </label>
);

export const Num = ({ label, value, onChange, hint, min, max, step }: { label: ReactNode; value: number | undefined | null; onChange: (v: number | undefined) => void; hint?: ReactNode; min?: number; max?: number; step?: number }) => (
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
  <Text label="Tags" value={(value ?? []).join(', ')} onChange={(v) => onChange(v.split(',').map((s) => s.trim()).filter(Boolean))} hint="comma-separated" />
);

export const CONDITION_HINT = (
  <>
    Rule language: <code>{'{"all":[…]}'}</code>, <code>{'{"any":[…]}'}</code>, <code>{'{"not":{…}}'}</code> or a comparison{' '}
    <code>{'{"prop":"country","op":"in","value":["DE","FR"]}'}</code>. Ops: eq neq gt gte lt lte in not_in contains not_contains starts_with ends_with matches exists not_exists semver_eq semver_gt semver_gte semver_lt semver_lte.
    Props: <code>default.*</code>, <code>custom.*</code>, <code>computed.*</code>. <code>{'{"all":[]}'}</code> matches everyone; leave the field empty for no condition where it is optional.
  </>
);
