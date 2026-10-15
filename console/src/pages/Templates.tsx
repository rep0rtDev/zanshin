import { useState } from 'react';
import { get, post, useLoad } from '../api';
import { EntityPage, Tags, Text } from '../crud';
import { Badge, ErrorBox, Field, JsonField, fmtDate } from '../ui';
import { t as tr } from '../i18n';

type Translation = { title: string; body: string };
type Template = {
  id: string; key: string; name: string; description: string; format: 'plaintext' | 'html' | 'json'; default_locale: string;
  translations: Record<string, Translation>; image_url: string; payload: unknown; tags: string[]; version: number; updated_at: string;
};

function TranslationsEditor({ value, defaultLocale, onChange }: { value: Record<string, Translation>; defaultLocale: string; onChange: (v: Record<string, Translation>) => void }) {
  const [adding, setAdding] = useState('');
  const locales = Object.keys(value);
  return (
    <div className="stack">
      {locales.map((l) => (
        <div key={l} className="card" style={{ padding: 10 }}>
          <div className="row" style={{ alignItems: 'center' }}>
            <code>{l}</code>
            {l === defaultLocale && <Badge>default</Badge>}
            <button className="btn ghost sm" style={{ marginLeft: 'auto' }} onClick={() => { const v = { ...value }; delete v[l]; onChange(v); }}>{tr('Remove')}</button>
          </div>
          <Text label={tr('Title')} value={value[l].title} onChange={(title) => onChange({ ...value, [l]: { ...value[l], title } })} placeholder="Welcome back, {{default.name}}!" />
          <Field label={tr('Body')}><textarea rows={3} value={value[l].body} onChange={(e) => onChange({ ...value, [l]: { ...value[l], body: e.target.value } })} style={{ fontFamily: 'inherit' }} /></Field>
        </div>
      ))}
      <div className="row" style={{ alignItems: 'end' }}>
        <Text label={tr('Add locale')} value={adding} onChange={setAdding} placeholder={tr('de, pt-BR, ja')} mono />
        <button className="btn ghost" disabled={!adding || !!value[adding]} onClick={() => { onChange({ ...value, [adding]: { title: '', body: '' } }); setAdding(''); }}>{tr('Add')}</button>
      </div>
    </div>
  );
}

function prettyJson(s: string) {
  try { return JSON.stringify(JSON.parse(s), null, 2); } catch { return s; }
}

function Preview({ t }: { t: Template }) {
  const [locale, setLocale] = useState(t.default_locale);
  const [ext, setExt] = useState('');
  const [out, setOut] = useState<{ rendered: { locale: string; title: string; body: string } } | null>(null);
  const [sent, setSent] = useState<string | null>(null);
  const [err, setErr] = useState<Error | null>(null);
  const k = encodeURIComponent(t.key);
  const body = () => ({ locale: locale || undefined, external_id: ext || undefined });
  return (
    <div className="card">
      <strong>{tr('Preview & test send')}</strong>
      <ErrorBox error={err} />
      <div className="row" style={{ marginTop: 8, alignItems: 'end' }}>
        <Field label={tr('Locale')}><select value={locale} onChange={(e) => setLocale(e.target.value)}>{Object.keys(t.translations).map((l) => <option key={l} value={l}>{l}</option>)}<option value="">{tr('player\'s own')}</option></select></Field>
        <Text label={tr('Player (external id)')} value={ext} onChange={setExt} mono hint={tr('optional for preview, required for test send')} />
        <button className="btn ghost" onClick={() => post<{ rendered: { locale: string; title: string; body: string } }>(`/templates/${k}/render`, body()).then((r) => { setOut(r); setErr(null); }, setErr)}>{tr('Render')}</button>
        <button className="btn" disabled={!ext} onClick={() => post<{ inbox_id: string }>(`/templates/${k}/test`, body()).then((r) => { setSent(r.inbox_id); setErr(null); }, setErr)}>{tr('Send test')}</button>
      </div>
      {out && (
        <div className="card" style={{ marginTop: 8 }}>
          <div className="hint">locale {out.rendered.locale}</div>
          <strong>{out.rendered.title}</strong>
          {t.format === 'html' ? <iframe className="rich-preview" title={tr('HTML preview')} sandbox="" srcDoc={out.rendered.body} /> : <pre style={{ whiteSpace: 'pre-wrap' }}>{t.format === 'json' ? prettyJson(out.rendered.body) : out.rendered.body}</pre>}
          <div className="hint">{new TextEncoder().encode(out.rendered.body).length} bytes</div>
        </div>
      )}
      {sent && <div className="hint" style={{ marginTop: 6 }}>Sent to inbox ({sent})</div>}
    </div>
  );
}

function UsedBy({ t }: { t: Template }) {
  const q = useLoad(() => get<{ used_by: string[] }>(`/templates/${encodeURIComponent(t.key)}`), [t.key]);
  return <div className="card"><strong>{tr('Used by')}</strong><div className="hint" style={{ marginTop: 6 }}>{q.data ? (q.data.used_by.length ? q.data.used_by.map((k) => <code key={k} style={{ marginRight: 8 }}>{k}</code>) : 'No campaign uses this template yet') : '…'}</div></div>;
}

export default function Templates() {
  return (
    <EntityPage<Template>
      title={tr('Templates')}
      path="/templates"
      blurb="Reusable message copy in plaintext, HTML or JSON with one translation per locale. Campaigns that reference a template render it in each player's locale (exact → language → default)."
      cols={[
        { h: 'Key', c: (t) => <><strong>{t.key}</strong><div className="hint">{t.name}</div></> },
        { h: 'Format', c: (t) => <Badge tone="info">{t.format}</Badge> },
        { h: 'Locales', c: (t) => Object.keys(t.translations).map((l) => <code key={l} style={{ marginRight: 6 }}>{l}</code>) },
        { h: 'Updated', c: (t) => fmtDate(t.updated_at) },
      ]}
      blank={() => ({ key: '', name: '', description: '', format: 'plaintext', default_locale: 'en', translations: { en: { title: '', body: '' } }, image_url: '', payload: null, tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label={tr('Key')} value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label={tr('Name')} value={d.name} onChange={(name) => set({ name })} />
          </div>
          <Text label={tr('Description')} value={d.description} onChange={(description) => set({ description })} />
          <div className="row">
            <Field label={tr('Format')}><select value={d.format} onChange={(e) => set({ format: e.target.value as Template['format'] })}><option value="plaintext">plaintext</option><option value="html">html</option><option value="json">json</option></select></Field>
            <Text label={tr('Default locale')} value={d.default_locale} onChange={(default_locale) => set({ default_locale })} mono />
            <Text label={tr('Image URL')} value={d.image_url} onChange={(image_url) => set({ image_url })} />
          </div>
          <Field label={tr('Translations')} hint="{{prop.path}} placeholders are substituted per player"><TranslationsEditor value={d.translations ?? {}} defaultLocale={d.default_locale ?? 'en'} onChange={(translations) => set({ translations })} /></Field>
          <Field label={tr('Payload (JSON)')} hint={tr('used when the campaign has no payload of its own')}><JsonField value={d.payload} onChange={(payload) => set({ payload: payload ?? null })} rows={2} /></Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(t) => <><Preview t={t} /><UsedBy t={t} /></>}
    />
  );
}
