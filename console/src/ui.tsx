import { useState } from 'react';
import type { ReactNode } from 'react';

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
  return <div className="error">{error.message}</div>;
}

export function Badge({ children, tone = 'muted' }: { children: ReactNode; tone?: 'ok' | 'warn' | 'bad' | 'muted' | 'info' }) {
  return <span className={'badge ' + tone}>{children}</span>;
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
        rows={rows}
        value={text}
        placeholder={placeholder}
        spellCheck={false}
        onChange={(e) => {
          setText(e.target.value);
          if (e.target.value.trim() === '') { setBad(false); onChange(undefined); return; }
          try { onChange(JSON.parse(e.target.value)); setBad(false); } catch { setBad(true); }
        }}
      />
      {bad && <span className="hint bad">invalid JSON</span>}
    </div>
  );
}

export type Col<T> = { h: ReactNode; c: (row: T) => ReactNode; w?: string };
export function Table<T>({ cols, rows, rowKey, empty = 'Nothing here yet', onRow }: { cols: Col<T>[]; rows: T[] | null | undefined; rowKey: (r: T) => string; empty?: ReactNode; onRow?: (r: T) => void }) {
  if (!rows) return <div className="muted">Loading…</div>;
  if (rows.length === 0) return <div className="muted empty">{empty}</div>;
  return (
    <table className="table">
      <thead><tr>{cols.map((c, i) => <th key={i} style={c.w ? { width: c.w } : undefined}>{c.h}</th>)}</tr></thead>
      <tbody>
        {rows.map((r) => (
          <tr key={rowKey(r)} className={onRow ? 'clickable' : undefined} onClick={onRow ? () => onRow(r) : undefined}>
            {cols.map((c, i) => <td key={i}>{c.c(r)}</td>)}
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
      <button className={'btn ' + (danger ? 'danger' : '')} onClick={() => { setArm(false); onConfirm(); }}>Confirm</button>
      <button className="btn ghost" onClick={() => setArm(false)}>Cancel</button>
    </span>
  );
}

export const fmtDate = (s?: string | null) => (s ? new Date(s).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '—');
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
