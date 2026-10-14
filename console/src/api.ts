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
  const res = await fetch('/admin/v1' + path, { method, headers, credentials: 'same-origin', body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await res.text();
  if (!res.ok) {
    let code = res.statusText, message = text;
    try {
      const e = JSON.parse(text);
      code = e.code ?? code;
      message = e.message ?? message;
    } catch { /* plain */ }
    if (res.status === 401) window.dispatchEvent(new Event('zanshin:unauthorized'));
    if (res.status === 403) window.dispatchEvent(new CustomEvent('zanshin:forbidden', { detail: message }));
    throw new ApiError(res.status, code, message);
  }
  return (text ? JSON.parse(text) : {}) as T;
}

/// Fetch a non-JSON admin resource (e.g. CSV) and save it through the browser.
export async function download(path: string, filename: string): Promise<void> {
  const headers: Record<string, string> = {};
  const k = auth.key();
  if (k) headers['x-api-key'] = k;
  const res = await fetch('/admin/v1' + path, { headers, credentials: 'same-origin' });
  if (!res.ok) throw new ApiError(res.status, res.statusText, await res.text());
  const url = URL.createObjectURL(await res.blob());
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
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

export type Role = 'viewer' | 'editor' | 'admin';
export type Me = { name: string; auth: 'api_key' | 'bootstrap' | 'oidc' | 'user'; role: Role; read_only: boolean; permissions: Record<string, ('read' | 'write' | 'delete')[]>; user_id: string | null; mfa_enabled: boolean; must_change_password: boolean };

/** Whether the server offers OIDC single sign-on for the console. */
export type AuthConfig = { oidc: boolean; password: boolean; needs_bootstrap: boolean };
export async function authConfig(): Promise<AuthConfig> {
  try {
    const r = await fetch('/console/auth/config', { credentials: 'same-origin' });
    return r.ok ? ((await r.json()) as AuthConfig) : { oidc: false, password: false, needs_bootstrap: false };
  } catch {
    return { oidc: false, password: false, needs_bootstrap: false };
  }
}

/** Email + password (+ one-time code) login; throws ApiError with code `mfa_required` when a code is needed. */
export async function passwordLogin(email: string, password: string, code?: string): Promise<void> {
  const r = await fetch('/console/auth/password', { method: 'POST', credentials: 'same-origin', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email, password, code: code || undefined }) });
  if (!r.ok) {
    const e = await r.json().catch(() => ({}));
    throw new ApiError(r.status, e.code ?? 'error', e.message ?? r.statusText);
  }
}

export async function resetPassword(token: string, password: string): Promise<void> {
  const r = await fetch('/console/auth/reset', { method: 'POST', credentials: 'same-origin', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ token, password }) });
  if (!r.ok) {
    const e = await r.json().catch(() => ({}));
    throw new ApiError(r.status, e.code ?? 'error', e.message ?? r.statusText);
  }
}

export async function ssoLogout(): Promise<void> {
  try {
    await fetch('/console/auth/logout', { method: 'POST', credentials: 'same-origin' });
  } catch { /* cookie may already be gone */ }
}
