import { intlLocale, t } from './i18n';
import { useState } from 'react';
import { get, qs, useLoad } from './api';
import { ErrorBox } from './ui';

type Occ = { key: string; name: string; enabled: boolean; start: string; end: string };

const pad = (n: number) => String(n).padStart(2, '0');
const dayKey = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const COLORS = ['#1a5fb4', '#1e8e3e', '#b26a00', '#8e24aa', '#c62828', '#00838f', '#5d4037', '#455a64'];

/** Month grid of live-event occurrences (server-computed, so cron schedules are exact). */
export function Calendar({ onPick }: { onPick?: (key: string) => void }) {
  const [month, setMonth] = useState(() => { const d = new Date(); return new Date(d.getFullYear(), d.getMonth(), 1); });
  const first = new Date(month.getFullYear(), month.getMonth(), 1);
  const gridStart = new Date(first); gridStart.setDate(1 - ((first.getDay() + 6) % 7));
  const gridEnd = new Date(gridStart); gridEnd.setDate(gridStart.getDate() + 42);
  const q = useLoad(() => get<{ occurrences: Occ[] }>('/live-events/occurrences' + qs({ from: gridStart.toISOString(), to: gridEnd.toISOString() })), [month.getTime()]);
  const keys = Array.from(new Set((q.data?.occurrences ?? []).map((o) => o.key)));
  const color = (k: string) => COLORS[keys.indexOf(k) % COLORS.length];
  const byDay = new Map<string, Occ[]>();
  for (const o of q.data?.occurrences ?? []) {
    const s = new Date(o.start), e = new Date(o.end);
    for (let d = new Date(s.getFullYear(), s.getMonth(), s.getDate()); d < e && d < gridEnd; d.setDate(d.getDate() + 1)) {
      if (d < gridStart) continue;
      const k = dayKey(d);
      byDay.set(k, [...(byDay.get(k) ?? []), o]);
    }
  }
  const days: Date[] = Array.from({ length: 42 }, (_, i) => { const d = new Date(gridStart); d.setDate(gridStart.getDate() + i); return d; });
  const today = dayKey(new Date());
  return (
    <div className="card cal">
      <div className="cal-head">
        <button className="btn ghost sm" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() - 1, 1))}>‹</button>
        <strong>{month.toLocaleDateString(intlLocale, { month: 'long', year: 'numeric' })}</strong>
        <button className="btn ghost sm" onClick={() => setMonth(new Date(month.getFullYear(), month.getMonth() + 1, 1))}>›</button>
        <button className="btn ghost sm" onClick={() => { const d = new Date(); setMonth(new Date(d.getFullYear(), d.getMonth(), 1)); }}>today</button>
        <span className="spacer" />
        <span className="hint">{q.data?.occurrences.length ?? 0} {t('occurrences · times shown in your local zone')}</span>
      </div>
      <ErrorBox error={q.error} />
      <div className="cal-grid">
        {['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'].map((d) => <div key={d} className="cal-dow">{d}</div>)}
        {days.map((d) => {
          const k = dayKey(d);
          const occ = byDay.get(k) ?? [];
          const seen = new Set<string>();
          return (
            <div key={k} className={'cal-day' + (d.getMonth() !== month.getMonth() ? ' off' : '') + (k === today ? ' today' : '')}>
              <div className="cal-num">{d.getDate()}</div>
              {occ.filter((o) => (seen.has(o.key + o.start) ? false : (seen.add(o.key + o.start), true))).slice(0, 4).map((o) => (
                <button key={o.key + o.start} className={'cal-ev' + (o.enabled ? '' : ' off')} style={{ background: color(o.key) }} title={`${o.name}\n${new Date(o.start).toLocaleString()} → ${new Date(o.end).toLocaleString()}`} onClick={() => onPick?.(o.key)}>
                  {new Date(o.start) >= d && dayKey(new Date(o.start)) === k ? `${pad(new Date(o.start).getHours())}:${pad(new Date(o.start).getMinutes())} ` : ''}{o.name || o.key}
                </button>
              ))}
              {occ.length > 4 && <div className="hint">+{occ.length - 4} more</div>}
            </div>
          );
        })}
      </div>
    </div>
  );
}
