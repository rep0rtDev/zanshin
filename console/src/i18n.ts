import { ru } from './locales/ru';

export type Locale = 'en' | 'ru';
export const LOCALES: [Locale, string][] = [['en', 'English'], ['ru', 'Русский']];
const KEY = 'zanshin.locale';

export function getLocale(): Locale {
  const s = localStorage.getItem(KEY);
  if (s === 'ru' || s === 'en') return s;
  return (navigator.language || '').toLowerCase().startsWith('ru') ? 'ru' : 'en';
}

/** Persists the choice and reloads: every string is resolved once at module load, so a reload is the cheapest consistent switch. */
export function setLocale(l: Locale) {
  localStorage.setItem(KEY, l);
  location.reload();
}

export const locale: Locale = getLocale();
document.documentElement.lang = locale;
export const intlLocale = locale === 'ru' ? 'ru-RU' : 'en-US';
const dict: Record<string, string> = locale === 'ru' ? ru : {};

/** Looks up an English UI string; falls back to the key itself so untranslated text stays English. */
export function t(s: string): string {
  return dict[s] ?? s;
}

/** Translates a template with `{0}`, `{1}`… placeholders, then substitutes the values. */
export function tf(s: string, ...args: unknown[]): string {
  return t(s).replace(/\{(\d+)\}/g, (_, i) => String(args[Number(i)] ?? ''));
}
