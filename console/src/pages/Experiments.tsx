import { useState } from 'react';
import { del, download, get, post, put, useLoad } from '../api';
import { DateTime, EntityPage, Num, Tags, Text } from '../crud';
import { AudiencePicker, ConditionEditor, VariantsEditor } from '../RuleBuilder';
import type { Cond } from '../RuleBuilder';
import { Badge, Confirm, ErrorBox, Field, KeyPicker, Table, fmtDate, fmtNum } from '../ui';
import { t, tf } from '../i18n';

type Variant = { key: string; name: string; value: unknown; weight: number };
type Guardrail = { metric: string; max_drop: number; min_identities: number };
type Bandit = { metric: string; min_weight: number; every_secs: number; min_assigned: number; last_run_at?: string | null; last_probabilities?: number[] };
type Holdout = { key: string; name: string; description: string; percentage: number; enabled: boolean; version: number };
type Experiment = {
  layer?: string | null; holdout?: string | null; guardrails?: Guardrail[]; bandit?: Bandit | null;
  split_key?: string; max_participants?: number | null; enrol_until?: string | null; locked?: boolean; metrics?: string[]; copied_from?: string | null; phase_of?: string | null; winner?: string | null; interrupt_reason?: string;
  key: string; name: string; description: string; hypothesis: string; status: 'draft' | 'running' | 'paused' | 'stopped';
  condition: Cond | null; audiences: string[]; percentage: number; variants: Variant[]; control: string | null; flag_key: string | null;
  starts_at: string | null; ends_at: string | null; tags: string[]; version: number; updated_at: string;
};
type VStats = { rate: number; ci_low: number; ci_high: number; prob_beats_control?: number; expected_uplift?: number };
type SeqStats = { delta: number; z: number; p_value: number; significant: boolean; lift?: number };
type CupedRow = { metric: string; name?: string; error?: string; theta?: number; variance_reduction?: number; players?: number; variants?: { variant: string; players: number; mean: number; adjusted_mean: number; adjusted_lift: number | null }[] };
type CatalogMetric = { metric: string; name?: string; kind?: string; aggregation?: string; error?: string; variants?: { variant: string; value: number; lift: number | null; stats?: VStats; sequential?: SeqStats | null }[] };
type Results = {
  cuped?: CupedRow[] | null;
  srm?: { chi2: number; p_value: number; suspicious: boolean } | null;
  metrics?: CatalogMetric[]; winner?: string | null; interrupted_at?: string | null; interrupt_reason?: string; locked?: boolean;
  total_assigned: number; metric: string | null; control: string | null;
  variants: { variant: string; weight: number; assigned: number; exposed: number; exposures: number; metric?: { identities: number; count: number; sum: number; conversion: number } }[];
};

const TONE: Record<Experiment['status'], 'muted' | 'ok' | 'warn' | 'bad'> = { draft: 'muted', running: 'ok', paused: 'warn', stopped: 'bad' };

function Lifecycle({ exp, reload }: { exp: Experiment; reload: () => void }) {
  const [err, setErr] = useState<Error | null>(null);
  const act = (a: string) => post(`/experiments/${encodeURIComponent(exp.key)}/${a}`).then(() => { setErr(null); reload(); }, setErr);
  return (
    <div className="card">
      <div className="inline" style={{ justifyContent: 'space-between', display: 'flex' }}>
        <strong>{t('Lifecycle')}</strong>
        <Badge tone={TONE[exp.status]}>{exp.status}</Badge>
      </div>
      <ErrorBox error={err} />
      <div className="actions" style={{ marginTop: 10 }}>
        {(exp.status === 'draft' || exp.status === 'paused') && <button className="btn" onClick={() => act('start')}>{t('Start')}</button>}
        {exp.status === 'running' && <button className="btn ghost" onClick={() => act('pause')}>{t('Pause')}</button>}
        {(exp.status === 'running' || exp.status === 'paused') && <button className="btn ghost" onClick={() => act('stop')}>{t('Stop')}</button>}
        {exp.status !== 'running' && <button className="btn danger ghost" onClick={() => act('reset')}>{t('Reset assignments')}</button>}
      </div>
      <div className="hint" style={{ marginTop: 8 }}>{t('Assignments are sticky: an identity keeps its variant while the experiment runs. Stopping keeps results; reset wipes assignments so the experiment can be re-run.')}</div>
    </div>
  );
}

export function MetricsPicker({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const q = useLoad(() => get<{ items: { key: string; kind: string; name: string }[] }>('/metrics'), []);
  const opts = q.data ? q.data.items.map((m) => ({ key: m.key, name: m.name, hint: m.kind })) : null;
  return <KeyPicker value={value} onChange={onChange} options={opts} placeholder={t('Add metric…')} empty={t('No metrics in the catalog yet — create them under Metrics.')} />;
}

function Ops({ exp, reload }: { exp: Experiment; reload: () => void }) {
  const [err, setErr] = useState<Error | null>(null);
  const [copyKey, setCopyKey] = useState(exp.key + '_v2');
  const [winner, setWinner] = useState(exp.control ?? exp.variants[0]?.key ?? '');
  const [reason, setReason] = useState('');
  const k = encodeURIComponent(exp.key);
  const run = (p: Promise<unknown>) => p.then(() => { setErr(null); reload(); }, setErr);
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}>
        <strong>{t('Operations')}</strong>
        {exp.locked && <Badge tone="warn">locked</Badge>}
        {exp.winner && <Badge tone="ok">winner: {exp.winner}</Badge>}
        {exp.phase_of && <span className="hint">{t('phase after') + ' '}<code>{exp.phase_of}</code></span>}
        {exp.copied_from && !exp.phase_of && <span className="hint">{t('copied from') + ' '}<code>{exp.copied_from}</code></span>}
      </div>
      <ErrorBox error={err} />
      <div className="row" style={{ marginTop: 10, alignItems: 'end' }}>
        <button className="btn ghost" onClick={() => run(post(`/experiments/${k}/${exp.locked ? 'unlock' : 'lock'}`))}>{exp.locked ? t('Unlock') : t('Lock')}</button>
        <Field label={t('Copy as')}><input value={copyKey} onChange={(e) => setCopyKey(e.target.value)} className="mono" /></Field>
        <button className="btn ghost" disabled={!copyKey} onClick={() => run(post(`/experiments/${k}/copy`, { key: copyKey, as_phase: false }))}>{t('Copy')}</button>
        <button className="btn ghost" disabled={!copyKey} onClick={() => run(post(`/experiments/${k}/copy`, { key: copyKey, as_phase: true }))}>{t('Next phase')}</button>
      </div>
      {exp.status !== 'draft' && exp.status !== 'stopped' && (
        <div className="row" style={{ marginTop: 10, alignItems: 'end' }}>
          <Field label={t('Winner')}><select value={winner} onChange={(e) => setWinner(e.target.value)}><option value="">{t('— none —')}</option>{exp.variants.map((v) => <option key={v.key} value={v.key}>{v.key}</option>)}</select></Field>
          <Field label={t('Reason')}><input value={reason} onChange={(e) => setReason(e.target.value)} placeholder={t('b wins on D7 retention')} /></Field>
          <Confirm danger onConfirm={() => run(post(`/experiments/${k}/interrupt`, { reason, winner: winner || null, apply_to_flag: !!exp.flag_key && !!winner }))}>{exp.flag_key && winner ? t('Interrupt & roll winner to flag') : t('Interrupt')}</Confirm>
        </div>
      )}
      {exp.interrupt_reason && <div className="hint" style={{ marginTop: 8 }}>{t('Interrupted:')} {exp.interrupt_reason}</div>}
    </div>
  );
}

type Participant = { identity_id: string; external_id: string; variant: string; assigned_at: string; exposed_at: string | null; exposures: number };

function Participants({ exp }: { exp: Experiment }) {
  const k = encodeURIComponent(exp.key);
  const q = useLoad(() => get<{ items: Participant[]; total: number }>(`/experiments/${k}/participants?limit=200`), [exp.key, exp.version]);
  const s = useLoad(() => get<{ assignments: { variant: string; day: string; count: number }[] }>(`/experiments/${k}/series`), [exp.key, exp.version]);
  const days = Array.from(new Set((s.data?.assignments ?? []).map((a) => a.day))).sort();
  return (
    <div className="card">
      <div className="row" style={{ alignItems: 'center' }}>
        <strong>{t('Participants')}</strong>
        {q.data && <span className="hint">{fmtNum(q.data.total)} assigned{exp.max_participants ? tf(' of {0} max', fmtNum(exp.max_participants)) : ''}</span>}
        <button className="btn ghost" onClick={() => download(`/experiments/${k}/participants.csv`, `${exp.key}-participants.csv`).catch(() => undefined)}>{t('CSV')}</button>
      </div>
      <ErrorBox error={q.error ?? s.error} />
      {days.length > 0 && (
        <table className="table" style={{ marginTop: 8 }}>
          <thead><tr><th>{t('Day')}</th>{exp.variants.map((v) => <th key={v.key}>{v.key}</th>)}</tr></thead>
          <tbody>{days.map((d) => <tr key={d}><td>{d}</td>{exp.variants.map((v) => <td key={v.key}>{s.data?.assignments.find((a) => a.day === d && a.variant === v.key)?.count ?? 0}</td>)}</tr>)}</tbody>
        </table>
      )}
      <Table<Participant>
        rows={q.data?.items}
        rowKey={(p) => p.identity_id}
        empty={t('Nobody assigned yet')}
        cols={[
          { h: t('Player'), c: (p) => <span className="mono">{p.external_id}</span> },
          { h: t('Variant'), c: (p) => <code>{p.variant}</code> },
          { h: t('Assigned'), c: (p) => fmtDate(p.assigned_at) },
          { h: t('Exposed'), c: (p) => p.exposed_at ? `${fmtDate(p.exposed_at)} (${p.exposures}×)` : <span className="muted">{t('not yet')}</span> },
        ]}
      />
    </div>
  );
}

function HoldoutSelect({ value, onChange }: { value: string | null; onChange: (v: string | null) => void }) {
  const q = useLoad(() => get<{ items: Holdout[] }>('/holdouts'), []);
  const items = q.data?.items ?? [];
  return (
    <Field label={t('Holdout group')} hint={t('players in the group are never enrolled here')}>
      <select value={value ?? ''} onChange={(e) => onChange(e.target.value || null)}>
        <option value="">none</option>
        {items.map((h) => <option key={h.key} value={h.key}>{h.key} · {h.percentage}%{h.enabled ? '' : ' (disabled)'}</option>)}
        {value && !items.some((h) => h.key === value) && <option value={value}>{value} (missing)</option>}
      </select>
    </Field>
  );
}

function GuardrailsEditor({ value, onChange }: { value: Guardrail[]; onChange: (v: Guardrail[]) => void }) {
  const upd = (i: number, patch: Partial<Guardrail>) => onChange(value.map((g, j) => (j === i ? { ...g, ...patch } : g)));
  return (
    <div className="stack" style={{ gap: 6 }}>
      {value.map((g, i) => (
        <div className="row" key={i} style={{ alignItems: 'end' }}>
          <div className="grow"><MetricsPicker value={g.metric ? [g.metric] : []} onChange={(m) => upd(i, { metric: m[m.length - 1] ?? '' })} /></div>
          <Num label={t('Max drop %')} value={Math.round(g.max_drop * 1000) / 10} onChange={(v) => upd(i, { max_drop: (v ?? 0) / 100 })} min={0} max={100} />
          <Num label={t('Min players')} value={g.min_identities} onChange={(v) => upd(i, { min_identities: v ?? 0 })} min={0} />
          <button type="button" className="btn ghost sm" onClick={() => onChange(value.filter((_, j) => j !== i))}>remove</button>
        </div>
      ))}
      <div><button type="button" className="btn ghost sm" onClick={() => onChange([...value, { metric: '', max_drop: 0.05, min_identities: 100 }])}>{t('+ add guardrail')}</button></div>
    </div>
  );
}

function BanditEditor({ value, onChange }: { value: Bandit | null; onChange: (v: Bandit | null) => void }) {
  if (!value) return <button type="button" className="btn ghost sm" onClick={() => onChange({ metric: '', min_weight: 5, every_secs: 3600, min_assigned: 100 })}>{t('Enable bandit')}</button>;
  return (
    <div className="stack" style={{ gap: 6 }}>
      <div className="row" style={{ alignItems: 'end' }}>
        <div className="grow"><MetricsPicker value={value.metric ? [value.metric] : []} onChange={(m) => onChange({ ...value, metric: m[m.length - 1] ?? '' })} /></div>
        <Num label={t('Min weight %')} value={value.min_weight} onChange={(v) => onChange({ ...value, min_weight: v ?? 0 })} min={0} max={50} />
        <Num label={t('Every (s)')} value={value.every_secs} onChange={(v) => onChange({ ...value, every_secs: v ?? 60 })} min={60} />
        <Num label={t('Explore until N assigned')} value={value.min_assigned} onChange={(v) => onChange({ ...value, min_assigned: v ?? 0 })} min={0} />
        <button type="button" className="btn ghost sm" onClick={() => onChange(null)}>disable</button>
      </div>
      {value.last_run_at && <div className="hint">{t('last re-weighted')} {fmtDate(value.last_run_at)} {t('· P(best):')} {(value.last_probabilities ?? []).map((p) => (p * 100).toFixed(0) + '%').join(' / ')}</div>}
    </div>
  );
}

function SequentialSummary({ metrics, control }: { metrics: CatalogMetric[]; control: string | null }) {
  const rows = metrics.flatMap((m) => (m.variants ?? []).filter((v) => v.sequential && v.variant !== control).map((v) => ({ metric: m.metric, variant: v.variant, s: v.sequential! })));
  if (rows.length === 0) return null;
  return (
    <div className="hint" style={{ marginBottom: 8 }}>
      {t('Always-valid (sequential) test — safe to read at any time:')}{' '}
      {rows.map((r) => <span key={r.metric + r.variant} style={{ marginRight: 10 }}><code>{r.metric}</code> {r.variant}: <Badge tone={r.s.significant ? 'ok' : 'muted'}>p = {r.s.p_value < 0.001 ? r.s.p_value.toExponential(1) : r.s.p_value.toFixed(3)}{r.s.lift !== undefined ? tf(' · lift {0}%', (r.s.lift * 100).toFixed(1)) : ''}</Badge></span>)}
    </div>
  );
}

function CupedTable({ rows }: { rows: CupedRow[] }) {
  return (
    <table className="table" style={{ marginBottom: 10 }}>
      <thead><tr><th>{t('Metric')}</th><th>θ</th><th>{t('Variance ↓')}</th><th>{t('Variant')}</th><th>{t('Mean')}</th><th>{t('CUPED mean')}</th><th>{t('Adj. lift')}</th></tr></thead>
      <tbody>
        {rows.map((m) => m.error
          ? <tr key={m.metric}><td><strong>{m.metric}</strong></td><td colSpan={6} className="hint">{m.error}</td></tr>
          : (m.variants ?? []).map((v, i) => (
            <tr key={m.metric + v.variant}>
              {i === 0 && <><td rowSpan={m.variants!.length}><strong>{m.metric}</strong><div className="hint">{fmtNum(m.players ?? 0)} players</div></td><td rowSpan={m.variants!.length}>{m.theta?.toFixed(3)}</td><td rowSpan={m.variants!.length}>{((m.variance_reduction ?? 0) * 100).toFixed(1)}%</td></>}
              <td>{v.variant}</td><td>{v.mean.toFixed(4)}</td><td>{v.adjusted_mean.toFixed(4)}</td><td>{v.adjusted_lift === null ? '—' : (v.adjusted_lift * 100).toFixed(1) + '%'}</td>
            </tr>
          )))}
      </tbody>
    </table>
  );
}

/** Holdout groups: deterministic slices of players kept out of the experiments that reference them. */
export function HoldoutsCard() {
  const q = useLoad(() => get<{ items: Holdout[] }>('/holdouts'), []);
  const [d, setD] = useState({ key: '', name: '', percentage: 5 });
  const [err, setErr] = useState<Error | null>(null);
  const run = (p: Promise<unknown>) => p.then(() => { setErr(null); q.reload(); }, setErr);
  return (
    <div className="card">
      <strong>{t('Holdout groups')}</strong> <span className="hint">{t('a fixed share of players kept out of every experiment that references the group — the untouched baseline for measuring the long-term effect of experimentation')}</span>
      <ErrorBox error={err ?? q.error} />
      <Table<Holdout>
        rows={q.data?.items ?? []}
        rowKey={(h) => h.key}
        empty={t('No holdout groups yet.')}
        cols={[
          { h: t('Key'), c: (h) => <><strong>{h.key}</strong><div className="hint">{h.name}</div></> },
          { h: t('Held out'), c: (h) => `${h.percentage}%` },
          { h: t('Status'), c: (h) => <Badge tone={h.enabled ? 'ok' : 'muted'}>{h.enabled ? 'enabled' : 'disabled'}</Badge> },
          { h: '', c: (h) => <div className="actions"><button className="btn ghost sm" onClick={() => run(put(`/holdouts/${encodeURIComponent(h.key)}`, { enabled: !h.enabled }))}>{h.enabled ? 'disable' : 'enable'}</button><Confirm danger onConfirm={() => run(del(`/holdouts/${encodeURIComponent(h.key)}`))}>{t('Delete')}</Confirm></div> },
        ]}
      />
      <div className="row" style={{ alignItems: 'end', marginTop: 8 }}>
        <Text label={t('Key')} value={d.key} onChange={(key) => setD({ ...d, key })} mono />
        <Text label={t('Name')} value={d.name} onChange={(name) => setD({ ...d, name })} />
        <Num label={t('Held out %')} value={d.percentage} onChange={(v) => setD({ ...d, percentage: v ?? 0 })} min={0} max={100} />
        <button className="btn" disabled={!d.key} onClick={() => run(post('/holdouts', d)).then(() => setD({ key: '', name: '', percentage: 5 }))}>{t('Create')}</button>
      </div>
    </div>
  );
}

function ResultsPanel({ exp }: { exp: Experiment }) {
  const [metric, setMetric] = useState('');
  const [applied, setApplied] = useState('');
  const [cuped, setCuped] = useState(false);
  const q = useLoad(() => get<Results>(`/experiments/${encodeURIComponent(exp.key)}/results?${applied ? 'metric=' + encodeURIComponent(applied) + '&' : ''}${cuped ? 'cuped=true' : ''}`), [exp.key, applied, exp.version, cuped]);
  const r = q.data;
  const ctrl = r?.variants.find((v) => v.variant === r.control);
  return (
    <div className="card">
      <strong>{t('Results')}</strong>
      <ErrorBox error={q.error} />
      <div className="row" style={{ marginTop: 10 }}>
        <Field label={t('Conversion metric (event name)')}><input value={metric} onChange={(e) => setMetric(e.target.value)} placeholder="purchase" onKeyDown={(e) => e.key === 'Enter' && setApplied(metric)} /></Field>
        <div className="field fixed"><span className="label">&nbsp;</span><button className="btn ghost" onClick={() => setApplied(metric)}>{t('Apply')}</button></div>
      </div>
      {r && (
        <>
          <div className="hint" style={{ marginBottom: 8 }}>{fmtNum(r.total_assigned)} {t('identities assigned')}{r.metric ? tf(' · metric "{0}"', r.metric) : ''}
            {r.srm && <> {t('· sample ratio')} {r.srm.suspicious ? <Badge tone="bad">{t('mismatch (p =')} {r.srm.p_value.toExponential(1)}{t(') — assignment is skewed, do not trust these results')}</Badge> : <Badge tone="ok">{t('ok (p =')} {r.srm.p_value.toFixed(2)})</Badge>}</>}
            <button className="btn ghost sm" style={{ marginLeft: 8 }} onClick={() => download(`/experiments/${encodeURIComponent(exp.key)}/results.csv`, `${exp.key}-results.csv`)}>{t('Download CSV')}</button>
          </div>
          <SequentialSummary metrics={r.metrics ?? []} control={r.control} />
          <div className="row" style={{ alignItems: 'center', marginBottom: 8 }}>
            <label className="hint" style={{ display: 'flex', gap: 6, alignItems: 'center' }}><input type="checkbox" checked={cuped} onChange={(e) => setCuped(e.target.checked)} />{' ' + t('CUPED (variance reduction from pre-experiment behaviour, count / sum / avg metrics)')}</label>
          </div>
          {cuped && r.cuped && <CupedTable rows={r.cuped} />}

          {r.metrics && r.metrics.length > 0 && (

            <table className="table" style={{ marginBottom: 10 }}>

              <thead><tr><th>{t('Metric')}</th><th>{t('Kind')}</th>{r.variants.map((v) => <th key={v.variant}>{v.variant}{r.control === v.variant ? ' (control)' : ''}</th>)}</tr></thead>

              <tbody>{r.metrics.map((m) => <tr key={m.metric}><td><strong>{m.metric}</strong><div className="hint">{m.error ?? `${m.aggregation}`}</div></td><td>{m.kind && <Badge tone={m.kind === 'goal' ? 'ok' : 'info'}>{m.kind}</Badge>}</td>{r.variants.map((v) => { const x = m.variants?.find((y) => y.variant === v.variant); return <td key={v.variant}>{x ? <>{fmtNum(x.value)}{x.lift != null && <span className={'hint ' + (x.lift >= 0 ? 'up' : 'down')}> {x.lift >= 0 ? '+' : ''}{(x.lift * 100).toFixed(1)}%</span>}{x.stats && <div className="hint">{(x.stats.ci_low * 100).toFixed(1)}–{(x.stats.ci_high * 100).toFixed(1)}%{x.stats.prob_beats_control != null && <> {t('· P(beats control)')} <strong>{(x.stats.prob_beats_control * 100).toFixed(0)}%</strong></>}</div>}</> : '—'}</td>; })}</tr>)}</tbody>

            </table>

          )}
          <table className="table">
            <thead><tr><th>{t('Variant')}</th><th>{t('Weight')}</th><th>{t('Assigned')}</th><th>{t('Exposed')}</th>{r.metric && <><th>{t('Converted')}</th><th>{t('Conv. rate')}</th><th>{t('Lift vs control')}</th></>}</tr></thead>
            <tbody>
              {r.variants.map((v) => {
                const rate = v.metric ? v.metric.conversion : null;
                const cr = ctrl?.metric?.conversion;
                return (
                  <tr key={v.variant}>
                    <td><strong>{v.variant}</strong>{v.variant === r.control && <Badge>control</Badge>}</td>
                    <td>{v.weight}</td>
                    <td>{fmtNum(v.assigned)}<div className="bar" style={{ marginTop: 4 }}><i style={{ width: `${r.total_assigned ? (v.assigned / r.total_assigned) * 100 : 0}%` }} /></div></td>
                    <td>{fmtNum(v.exposed)} <span className="hint">({fmtNum(v.exposures)} exposures)</span></td>
                    {r.metric && <>
                      <td>{fmtNum(v.metric?.identities ?? 0)}</td>
                      <td>{rate !== null ? (rate * 100).toFixed(2) + '%' : '—'}</td>
                      <td>{rate !== null && cr !== undefined && cr > 0 && v.variant !== r.control ? ((rate / cr - 1) * 100).toFixed(1) + '%' : '—'}</td>
                    </>}
                  </tr>
                );
              })}
            </tbody>
          </table>
        </>
      )}
    </div>
  );
}

export default function Experiments() {
  return (
    <EntityPage<Experiment>
      title={t('Experiments')}
      path="/experiments"
      blurb={t('A/B tests with deterministic sticky assignment. Events are tagged with the active variant, so any event name can be used as a conversion metric.')}
      above={<HoldoutsCard />}
      cols={[
        { h: t('Key'), c: (e) => <><strong>{e.key}</strong><div className="hint">{e.name}</div></> },
        { h: t('Status'), c: (e) => <Badge tone={TONE[e.status]}>{e.status}</Badge> },
        { h: t('Variants'), c: (e) => e.variants.map((v) => v.key).join(' / ') },
        { h: t('Traffic'), c: (e) => `${e.percentage}%` },
        { h: t('Flag'), c: (e) => e.flag_key ? <code>{e.flag_key}</code> : <span className="muted">—</span> },
        { h: t('Updated'), c: (e) => fmtDate(e.updated_at) },
      ]}
      sort={(a, b) => (a.status === 'running' ? -1 : 1) - (b.status === 'running' ? -1 : 1) || a.key.localeCompare(b.key)}
      blank={() => ({ key: '', name: '', description: '', hypothesis: '', condition: null, audiences: [], percentage: 100, control: 'control', flag_key: null, starts_at: null, ends_at: null, tags: [], split_key: 'identity', max_participants: null, enrol_until: null, metrics: [],
        variants: [{ key: 'control', name: t('Control'), value: false, weight: 50 }, { key: 'treatment', name: t('Treatment'), value: true, weight: 50 }] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label={t('Key')} value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label={t('Name')} value={d.name} onChange={(name) => set({ name })} />
          </div>
          <Text label={t('Hypothesis')} value={d.hypothesis} onChange={(hypothesis) => set({ hypothesis })} placeholder={t('Showing the offer on day 2 increases D7 retention by 3%')} />
          <Text label={t('Description')} value={d.description} onChange={(description) => set({ description })} />
          <Field label={t('Variants')} hint={t('weights are relative; value is what the SDK receives (and what the linked flag resolves to)')}>
            <VariantsEditor type="json" withName value={(d.variants ?? []) as Variant[]} onChange={(variants) => set({ variants: variants as Variant[] })} />
          </Field>
          <div className="row">
            <Text label={t('Control variant')} value={d.control ?? ''} onChange={(v) => set({ control: v || null })} mono />
            <Text label={t('Linked flag key')} value={d.flag_key ?? ''} onChange={(v) => set({ flag_key: v || null })} mono hint={t('optional: the flag returns the assigned variant\'s value')} />
            <Num label={t('Traffic %')} value={d.percentage} onChange={(percentage) => set({ percentage: percentage ?? 0 })} min={0} max={100} />
          </div>
          <Field label={t('Targeting condition')}><ConditionEditor optional value={d.condition} onChange={(condition) => set({ condition })} /></Field>
          <Field label={t('Audiences')} hint={t('identity must belong to all selected')}><AudiencePicker value={d.audiences ?? []} onChange={(audiences) => set({ audiences })} /></Field>
          <div className="row">
            <DateTime label={t('Starts')} value={d.starts_at} onChange={(starts_at) => set({ starts_at })} />
            <DateTime label={t('Ends')} value={d.ends_at} onChange={(ends_at) => set({ ends_at })} />
            <DateTime label={t('Enrolment deadline')} value={d.enrol_until ?? null} onChange={(enrol_until) => set({ enrol_until })} />
          </div>
          <div className="row">
            <Num label={t('Max participants')} value={d.max_participants ?? null} onChange={(max_participants) => set({ max_participants: max_participants ?? null })} min={1} hint={t('blank = unlimited')} />
            <Text label={t('Split key')} value={d.split_key ?? 'identity'} onChange={(split_key) => set({ split_key })} mono hint={t('identity, or property:<path> e.g. property:custom.clan_id')} />
          </div>
          <div className="row">
            <Text label={t('Layer')} value={d.layer ?? ''} onChange={(v) => set({ layer: v || null })} mono hint={t('experiments in one layer are mutually exclusive; their traffic % must sum to ≤ 100')} />
            <HoldoutSelect value={d.holdout ?? null} onChange={(holdout) => set({ holdout })} />
          </div>
          <Field label={t('Guardrail metrics')} hint={t('auto-pause when a variant drops below control by more than the limit (checked every minute)')}>
            <GuardrailsEditor value={d.guardrails ?? []} onChange={(guardrails) => set({ guardrails })} />
          </Field>
          <Field label={t('Bandit allocation')} hint={t('Thompson sampling re-weights variants towards the one most likely to be best on a conversion metric; assigned players stay sticky')}>
            <BanditEditor value={d.bandit ?? null} onChange={(bandit) => set({ bandit })} />
          </Field>
          <Field label={t('Metrics')} hint={t('goal / monitor metrics from the catalog, evaluated per variant in results')}><MetricsPicker value={d.metrics ?? []} onChange={(metrics) => set({ metrics })} /></Field>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(e, reload) => (<><Lifecycle exp={e} reload={reload} /><Phases exp={e} /><Ops exp={e} reload={reload} /><ResultsPanel exp={e} /><Participants exp={e} /></>)}
    />
  );
}

type Phase = { phase: number; key: string; name: string; status: Experiment['status']; phase_of: string | null; starts_at: string | null; ends_at: string | null; audiences: string[]; variants: string[]; winner: string | null };

function Phases({ exp }: { exp: Experiment }) {
  const q = useLoad(() => get<{ items: Phase[] }>(`/experiments/${encodeURIComponent(exp.key)}/phases`), [exp.key, exp.version]);
  const items = q.data?.items ?? [];
  if (items.length < 2 && !exp.phase_of) return null;
  return (
    <div className="card">
      <strong>{t('Phases')}</strong> <span className="hint">{t('one family: a QA phase on a test audience, then the live phase — copy “as phase” to add the next one; edits to a template apply to the next phase, not the running one')}</span>
      <ErrorBox error={q.error} />
      <Table<Phase>
        rows={items}
        rowKey={(p) => p.key}
        empty="—"
        cols={[
          { h: '#', w: '50px', c: (p) => p.phase },
          { h: t('Experiment'), c: (p) => p.key === exp.key ? <strong>{p.key}</strong> : <a href={`/console/experiments/${encodeURIComponent(p.key)}`}>{p.key}</a> },
          { h: t('Status'), c: (p) => <Badge tone={TONE[p.status]}>{p.status}</Badge> },
          { h: t('Audiences'), c: (p) => p.audiences.length ? p.audiences.join(', ') : <span className="muted">everyone</span> },
          { h: t('Window'), c: (p) => <>{p.starts_at ? fmtDate(p.starts_at) : '—'} → {p.ends_at ? fmtDate(p.ends_at) : '—'}</> },
          { h: t('Winner'), c: (p) => p.winner ?? <span className="muted">—</span> },
        ]}
      />
    </div>
  );
}
