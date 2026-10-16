import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { intlLocale, t, tf } from './i18n';

export function Page({ title, actions, children }: { title: ReactNode; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="page">
      <header className="page-head">
        <h1>{title}</h1>
        <div className="actions">{actions}</div>
      </header>
      {children}
    </div>
  );
}

export function ErrorBox({ error }: { error: Error | null | undefined }) {
  if (!error) return null;
  const status = (error as { status?: number }).status;
  return <div className="error">{error.message || (status ? tf('Request failed ({0})', status) : t('Request failed'))}</div>;
}

export function Badge({ children, tone = 'muted' }: { children: ReactNode; tone?: 'ok' | 'warn' | 'bad' | 'muted' | 'info' }) {
  return <span className={'badge ' + tone}>{typeof children === 'string' ? t(children) : children}</span>;
}

export function Field({ label, hint, children }: { label: ReactNode; hint?: ReactNode; children: ReactNode }) {
  return (
    <label className="field">
      <span className="label">{label}</span>
      {children}
      {hint && <span className="hint">{hint}</span>}
    </label>
  );
}

export function Toggle({ value, onChange, label }: { value: boolean; onChange: (v: boolean) => void; label?: ReactNode }) {
  return (
    <button type="button" className={'toggle' + (value ? ' on' : '')} onClick={() => onChange(!value)} aria-pressed={value}>
      <span className="knob" />
      {label && <span>{label}</span>}
    </button>
  );
}

export function JsonField({ value, onChange, rows = 6, placeholder }: { value: unknown; onChange: (v: unknown) => void; rows?: number; placeholder?: string }) {
  const [text, setText] = useState(() => (value === undefined ? '' : JSON.stringify(value, null, 2)));
  const [bad, setBad] = useState(false);
  return (
    <div className={'json' + (bad ? ' bad' : '')}>
      <textarea
        rows={Math.min(Math.max(3, text.split('\n').length + 1), Math.max(rows, 12))}
        value={text}
        placeholder={placeholder}
        spellCheck={false}
        onChange={(e) => {
          setText(e.target.value);
          if (e.target.value.trim() === '') { setBad(false); onChange(undefined); return; }
          try { onChange(JSON.parse(e.target.value)); setBad(false); } catch { setBad(true); }
        }}
      />
      {bad && <span className="hint bad">{t('invalid JSON')}</span>}
    </div>
  );
}

export type Col<T> = { h: ReactNode; c: (row: T) => ReactNode; w?: string };
export function Table<T>({ cols, rows, rowKey, empty = t('Nothing here yet'), onRow }: { cols: Col<T>[]; rows: T[] | null | undefined; rowKey: (r: T) => string; empty?: ReactNode; onRow?: (r: T) => void }) {
  if (!rows) return <div className="muted">{t('Loading…')}</div>;
  if (rows.length === 0) return <div className="muted empty">{empty}</div>;
  return (
    <table className="table stack">
      <thead><tr>{cols.map((c, i) => <th key={i} style={c.w ? { width: c.w } : undefined}>{c.h}</th>)}</tr></thead>
      <tbody>
        {rows.map((r) => (
          <tr key={rowKey(r)} className={onRow ? 'clickable' : undefined} onClick={onRow ? () => onRow(r) : undefined}>
            {cols.map((c, i) => <td key={i} data-h={typeof c.h === 'string' ? c.h : undefined}>{c.c(r)}</td>)}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function Stat({ label, value, sub }: { label: ReactNode; value: ReactNode; sub?: ReactNode }) {
  return (
    <div className="stat">
      <div className="stat-v">{value}</div>
      <div className="stat-l">{label}</div>
      {sub && <div className="hint">{sub}</div>}
    </div>
  );
}

export function Confirm({ onConfirm, children, danger }: { onConfirm: () => void; children: ReactNode; danger?: boolean }) {
  const [arm, setArm] = useState(false);
  if (!arm) return <button className={danger ? 'btn danger ghost' : 'btn ghost'} onClick={() => setArm(true)}>{children}</button>;
  return (
    <span className="confirm">
      <button className={'btn ' + (danger ? 'danger' : '')} onClick={() => { setArm(false); onConfirm(); }}>{t('Confirm')}</button>
      <button className="btn ghost" onClick={() => setArm(false)}>{t('Cancel')}</button>
    </span>
  );
}

const TZ_KEY = 'zanshin.tz';
let displayTz: string | undefined = localStorage.getItem(TZ_KEY) || undefined;
/** Console-wide display timezone (Settings → General); empty = the browser's. Invalid names fall back silently. */
export const setDisplayTz = (tz: string) => {
  try { new Intl.DateTimeFormat(undefined, { timeZone: tz || undefined }); } catch { tz = ''; }
  displayTz = tz || undefined;
  if (tz) localStorage.setItem(TZ_KEY, tz); else localStorage.removeItem(TZ_KEY);
};
export const displayTimezone = () => displayTz ?? Intl.DateTimeFormat().resolvedOptions().timeZone;
export const fmtDate = (s?: string | null) => (s ? new Date(s).toLocaleString(intlLocale, { dateStyle: 'medium', timeStyle: 'short', timeZone: displayTz }) : '—');
export const fmtNum = (n: number) => new Intl.NumberFormat().format(n);
export const short = (s: string, n = 8) => (s.length > n ? s.slice(0, n) + '…' : s);

export function Sparkline({ points, height = 48 }: { points: number[]; height?: number }) {
  if (points.length < 2) return <div className="muted">—</div>;
  const w = 100, max = Math.max(...points, 1);
  const d = points.map((p, i) => `${(i / (points.length - 1)) * w},${height - (p / max) * (height - 4) - 2}`).join(' ');
  return (
    <svg viewBox={`0 0 ${w} ${height}`} preserveAspectRatio="none" className="spark" style={{ height }}>
      <polyline points={d} fill="none" stroke="currentColor" strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

export function Menu({ items, label = '⋯' }: { items: { label: ReactNode; onClick: () => void; danger?: boolean; disabled?: boolean }[]; label?: ReactNode }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const h = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false); };
    document.addEventListener('mousedown', h);
    return () => document.removeEventListener('mousedown', h);
  }, [open]);
  return (
    <div className="menu" ref={ref} onClick={(e) => e.stopPropagation()}>
      <button className="btn ghost sm" onClick={() => setOpen((o) => !o)} aria-label={t('More')}>{label}</button>
      {open && (
        <div className="menu-pop">
          {items.map((it, i) => <button key={i} className={'menu-item' + (it.danger ? ' danger' : '')} disabled={it.disabled} onClick={() => { setOpen(false); it.onClick(); }}>{it.label}</button>)}
        </div>
      )}
    </div>
  );
}

export function Empty({ title, hint, action }: { title: ReactNode; hint?: ReactNode; action?: ReactNode }) {
  return <div className="empty"><div style={{ fontWeight: 600 }}>{title}</div>{hint && <div className="hint" style={{ marginTop: 4 }}>{hint}</div>}{action && <div style={{ marginTop: 12 }}>{action}</div>}</div>;
}

export function Pager({ page, pages, size, onPage, onSize, total }: { page: number; pages: number; size: number; onPage: (p: number) => void; onSize: (s: number) => void; total: number }) {
  if (total <= 25) return null;
  return (
    <div className="pager">
      <span className="hint">{tf('{0} total', fmtNum(total))}</span>
      <select value={size} onChange={(e) => onSize(Number(e.target.value))} style={{ width: 'auto' }}>{[25, 50, 100].map((n) => <option key={n} value={n}>{tf('{0} / page', n)}</option>)}</select>
      <button className="btn ghost sm" disabled={page <= 0} onClick={() => onPage(page - 1)}>‹</button>
      <span className="hint">{page + 1} / {pages}</span>
      <button className="btn ghost sm" disabled={page >= pages - 1} onClick={() => onPage(page + 1)}>›</button>
    </div>
  );
}

export type PickOption = { key: string; name?: string; hint?: ReactNode };
/** Multi-select over a catalog: chosen keys as removable chips + a search box with a dropdown. Replaces chip walls. */
export function KeyPicker({ value, onChange, options, placeholder = t('Search…'), empty }: { value: string[]; onChange: (v: string[]) => void; options: PickOption[] | null; placeholder?: string; empty?: ReactNode }) {
  const [q, setQ] = useState('');
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const h = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false); };
    document.addEventListener('mousedown', h);
    return () => document.removeEventListener('mousedown', h);
  }, [open]);
  if (!options) return <span className="hint">{t('Loading…')}</span>;
  if (!options.length && !value.length) return <span className="hint">{empty ?? t('Nothing to pick from yet')}</span>;
  const s = q.trim().toLowerCase();
  const rest = options.filter((o) => !value.includes(o.key) && (!s || `${o.key} ${o.name ?? ''}`.toLowerCase().includes(s))).slice(0, 8);
  const add = (k: string) => { onChange([...value, k]); setQ(''); };
  return (
    <div className="picker" ref={ref}>
      <div className="picker-box" onClick={() => setOpen(true)}>
        {value.map((k) => {
          const o = options.find((x) => x.key === k);
          return (
            <span key={k} className={'chip on' + (o ? '' : ' missing')} title={o?.name ?? 'unknown'}>
              {k}<button type="button" className="chip-x" aria-label={tf('Remove {0}', k)} onClick={(e) => { e.stopPropagation(); onChange(value.filter((x) => x !== k)); }}>×</button>
            </span>
          );
        })}
        <input
          className="picker-input"
          value={q}
          placeholder={value.length ? '' : placeholder}
          onFocus={() => setOpen(true)}
          onChange={(e) => { setQ(e.target.value); setOpen(true); }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && rest[0]) { e.preventDefault(); add(rest[0].key); }
            if (e.key === 'Backspace' && !q && value.length) onChange(value.slice(0, -1));
            if (e.key === 'Escape') setOpen(false);
          }}
        />
      </div>
      {open && (
        <div className="picker-pop">
          {rest.length === 0 ? <div className="hint" style={{ padding: '6px 10px' }}>{s ? t('No matches') : t('All selected')}</div> : rest.map((o) => (
            <button key={o.key} type="button" className="menu-item" onMouseDown={(e) => e.preventDefault()} onClick={() => add(o.key)}>
              <span className="mono">{o.key}</span>{o.name && o.name !== o.key && <span className="hint" style={{ marginLeft: 8 }}>{o.name}</span>}{o.hint && <span className="hint" style={{ marginLeft: 8 }}>{o.hint}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
