import { useEffect, useState } from 'react';

type Toast = { id: number; text: string; tone: 'ok' | 'bad' | 'info' };
const listeners = new Set<(t: Toast) => void>();
let seq = 0;
export const toast = (text: string, tone: Toast['tone'] = 'ok') => { const t = { id: ++seq, text, tone }; listeners.forEach((l) => l(t)); };

export function Toasts() {
  const [items, setItems] = useState<Toast[]>([]);
  useEffect(() => {
    const l = (t: Toast) => { setItems((xs) => [...xs, t]); setTimeout(() => setItems((xs) => xs.filter((x) => x.id !== t.id)), t.tone === 'bad' ? 6000 : 2600); };
    listeners.add(l);
    return () => { listeners.delete(l); };
  }, []);
  if (!items.length) return null;
  return <div className="toasts">{items.map((t) => <div key={t.id} className={'toast ' + t.tone} onClick={() => setItems((xs) => xs.filter((x) => x.id !== t.id))}>{t.text}</div>)}</div>;
}
