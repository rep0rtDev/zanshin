import { useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { get } from './api';

export type PaletteItem = { label: string; sub?: string; to: string; group: string };

const SOURCES: [string, string, string][] = [
  ['/flags', 'Flags', '/flags'], ['/audiences', 'Audiences', '/audiences'], ['/experiments', 'Experiments', '/experiments'],
  ['/live-events', 'Live events', '/live-events'], ['/messages', 'Messages', '/messages'], ['/templates', 'Templates', '/templates'],
  ['/journeys', 'Journeys', '/journeys'], ['/integrations', 'Integrations', '/integrations'], ['/webhooks', 'Webhooks', '/webhooks'],
  ['/metrics', 'Metrics', '/metrics'], ['/export-sinks', 'Export sinks', '/data'], ['/dashboards', 'Dashboards', '/insights'],
];

export function Palette({ pages, open, onClose }: { pages: PaletteItem[]; open: boolean; onClose: () => void }) {
  const nav = useNavigate();
  const [q, setQ] = useState('');
  const [items, setItems] = useState<PaletteItem[]>([]);
  const [sel, setSel] = useState(0);
  const inp = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!open) return;
    setQ(''); setSel(0);
    setTimeout(() => inp.current?.focus(), 0);
    Promise.all(SOURCES.map(([p, g, to]) => get<Array<{ key: string; name?: string; status?: string }>>(p).then((xs) => (Array.isArray(xs) ? xs : []).map((x) => ({ label: x.key, sub: [x.name, x.status].filter(Boolean).join(' · '), to: `${to}/${encodeURIComponent(x.key)}`, group: g })), () => [] as PaletteItem[])))
      .then((all) => setItems(all.flat()));
  }, [open]);
  const all = useMemo(() => [...pages, ...items], [pages, items]);
  const hits = useMemo(() => {
    const s = q.trim().toLowerCase();
    if (!s) return pages.slice(0, 12);
    const score = (it: PaletteItem) => { const l = it.label.toLowerCase(); return l === s ? 0 : l.startsWith(s) ? 1 : l.includes(s) ? 2 : (it.sub ?? '').toLowerCase().includes(s) ? 3 : it.group.toLowerCase().includes(s) ? 4 : 9; };
    return all.map((it) => [score(it), it] as const).filter(([sc]) => sc < 9).sort((a, b) => a[0] - b[0]).slice(0, 14).map(([, it]) => it);
  }, [q, all, pages]);
  useEffect(() => setSel(0), [q]);
  if (!open) return null;
  const go = (it: PaletteItem) => { onClose(); nav(it.to); };
  return (
    <div className="palette-bg" onMouseDown={onClose}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()}>
        <input ref={inp} value={q} onChange={(e) => setQ(e.target.value)} placeholder="Jump to a page or entity…"
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown') { e.preventDefault(); setSel((s) => Math.min(s + 1, hits.length - 1)); }
            else if (e.key === 'ArrowUp') { e.preventDefault(); setSel((s) => Math.max(s - 1, 0)); }
            else if (e.key === 'Enter' && hits[sel]) go(hits[sel]);
            else if (e.key === 'Escape') onClose();
          }} />
        <div className="palette-list">
          {hits.length === 0 && <div className="hint" style={{ padding: 12 }}>No matches</div>}
          {hits.map((it, i) => (
            <div key={it.group + it.to} className={'palette-item' + (i === sel ? ' on' : '')} onMouseEnter={() => setSel(i)} onClick={() => go(it)}>
              <span className="palette-group">{it.group}</span>
              <span className="trunc">{it.label}</span>
              {it.sub && <span className="hint trunc">{it.sub}</span>}
            </div>
          ))}
        </div>
        <div className="palette-foot hint">↑↓ navigate · ↵ open · esc close</div>
      </div>
    </div>
  );
}
