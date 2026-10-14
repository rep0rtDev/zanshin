import { useCallback, useEffect, useState } from 'react';

const KEY = 'zanshin.adminKey';
export const auth = {
  key: () => localStorage.getItem(KEY),
  set: (k: string) => localStorage.setItem(KEY, k),
  clear: () => localStorage.removeItem(KEY),
};

export class ApiError extends Error {
  status: number;
  code: string;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

export async function api<T = unknown>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { accept: 'application/json' };
  const k = auth.key();
  if (k) headers['x-api-key'] = k;
  if (body !== undefined) headers['content-type'] = 'application/json';
  const res = await fetch('/admin/v1' + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await res.text();
  if (!res.ok) {
    let code = res.statusText, message = text;
    try {
      const e = JSON.parse(text);
      code = e.code ?? code;
      message = e.message ?? message;
    } catch { /* plain */ }
    if (res.status === 401) window.dispatchEvent(new Event('zanshin:unauthorized'));
    throw new ApiError(res.status, code, message);
  }
  return (text ? JSON.parse(text) : {}) as T;
}

export const get = <T,>(p: string) => api<T>('GET', p);
export const post = <T,>(p: string, b?: unknown) => api<T>('POST', p, b);
export const put = <T,>(p: string, b?: unknown) => api<T>('PUT', p, b);
export const del = <T,>(p: string) => api<T>('DELETE', p);

export function useLoad<T>(fn: () => Promise<T>, deps: unknown[] = []) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const reload = useCallback(() => setTick((t) => t + 1), []);
  useEffect(() => {
    let alive = true;
    setLoading(true);
    fn().then((d) => alive && (setData(d), setError(null)), (e) => alive && setError(e)).finally(() => alive && setLoading(false));
    return () => { alive = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  return { data, error, loading, reload, setData };
}

export function qs(params: Record<string, string | number | boolean | undefined | null>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== '') q.set(k, String(v));
  const s = q.toString();
  return s ? '?' + s : '';
}
