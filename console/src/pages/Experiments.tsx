import { useState } from 'react';
import { download, get, post, useLoad } from '../api';
import { DateTime, EntityPage, Num, Tags, Text } from '../crud';
import { AudiencePicker, ConditionEditor, VariantsEditor } from '../RuleBuilder';
import type { Cond } from '../RuleBuilder';
import { Badge, Confirm, ErrorBox, Field, KeyPicker, Table, fmtDate, fmtNum } from '../ui';

type Variant = { key: string; name: string; value: unknown; weight: number };
type Experiment = {
  split_key?: string; max_participants?: number | null; enrol_until?: string | null; locked?: boolean; metrics?: string[]; copied_from?: string | null; phase_of?: string | null; winner?: string | null; interrupt_reason?: string;
  key: string; name: string; description: string; hypothesis: string; status: 'draft' | 'running' | 'paused' | 'stopped';
  condition: Cond | null; audiences: string[]; percentage: number; variants: Variant[]; control: string | null; flag_key: string | null;
  starts_at: string | null; ends_at: string | null; tags: string[]; version: number; updated_at: string;
};
type VStats = { rate: number; ci_low: number; ci_high: number; prob_beats_control?: number; expected_uplift?: number };
type CatalogMetric = { metric: string; name?: string; kind?: string; aggregation?: string; error?: string; variants?: { variant: string; value: number; lift: number | null; stats?: VStats }[] };
type Results = {
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
        <strong>Lifecycle</strong>
        <Badge tone={TONE[exp.status]}>{exp.status}</Badge>
      </div>
      <ErrorBox error={err} />
      <div className="actions" style={{ marginTop: 10 }}>
        {(exp.status === 'draft' || exp.status === 'paused') && <button className="btn" onClick={() => act('start')}>Start</button>}
        {exp.status === 'running' && <button className="btn ghost" onClick={() => act('pause')}>Pause</button>}
        {(exp.status === 'running' || exp.status === 'paused') && <button className="btn ghost" onClick={() => act('stop')}>Stop</button>}
        {exp.status !== 'running' && <button className="btn danger ghost" onClick={() => act('reset')}>Reset assignments</button>}
      </div>
      <div className="hint" style={{ marginTop: 8 }}>Assignments are sticky: an identity keeps its variant while the experiment runs. Stopping keeps results; reset wipes assignments so the experiment can be re-run.</div>
    </div>
  );
}

export function MetricsPicker({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const q = useLoad(() => get<{ items: { key: string; kind: string; name: string }[] }>('/metrics'), []);
  const opts = q.data ? q.data.items.map((m) => ({ key: m.key, name: m.name, hint: m.kind })) : null;
  return <KeyPicker value={value} onChange={onChange} options={opts} placeholder="Add metric…" empty="No metrics in the catalog yet — create them under Metrics." />;
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
        <strong>Operations</strong>
        {exp.locked && <Badge tone="warn">locked</Badge>}
        {exp.winner && <Badge tone="ok">winner: {exp.winner}</Badge>}
        {exp.phase_of && <span className="hint">phase after <code>{exp.phase_of}</code></span>}
        {exp.copied_from && !exp.phase_of && <span className="hint">copied from <code>{exp.copied_from}</code></span>}
      </div>
      <ErrorBox error={err} />
      <div className="row" style={{ marginTop: 10, alignItems: 'end' }}>
        <button className="btn ghost" onClick={() => run(post(`/experiments/${k}/${exp.locked ? 'unlock' : 'lock'}`))}>{exp.locked ? 'Unlock' : 'Lock'}</button>
        <Field label="Copy as"><input value={copyKey} onChange={(e) => setCopyKey(e.target.value)} className="mono" /></Field>
        <button className="btn ghost" disabled={!copyKey} onClick={() => run(post(`/experiments/${k}/copy`, { key: copyKey, as_phase: false }))}>Copy</button>
        <button className="btn ghost" disabled={!copyKey} onClick={() => run(post(`/experiments/${k}/copy`, { key: copyKey, as_phase: true }))}>Next phase</button>
      </div>
      {exp.status !== 'draft' && exp.status !== 'stopped' && (
        <div className="row" style={{ marginTop: 10, alignItems: 'end' }}>
          <Field label="Winner"><select value={winner} onChange={(e) => setWinner(e.target.value)}><option value="">— none —</option>{exp.variants.map((v) => <option key={v.key} value={v.key}>{v.key}</option>)}</select></Field>
          <Field label="Reason"><input value={reason} onChange={(e) => setReason(e.target.value)} placeholder="b wins on D7 retention" /></Field>
          <Confirm danger onConfirm={() => run(post(`/experiments/${k}/interrupt`, { reason, winner: winner || null, apply_to_flag: !!exp.flag_key && !!winner }))}>{exp.flag_key && winner ? 'Interrupt & roll winner to flag' : 'Interrupt'}</Confirm>
        </div>
      )}
      {exp.interrupt_reason && <div className="hint" style={{ marginTop: 8 }}>Interrupted: {exp.interrupt_reason}</div>}
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
        <strong>Participants</strong>
        {q.data && <span className="hint">{fmtNum(q.data.total)} assigned{exp.max_participants ? ` of ${fmtNum(exp.max_participants)} max` : ''}</span>}
        <button className="btn ghost" onClick={() => download(`/experiments/${k}/participants.csv`, `${exp.key}-participants.csv`).catch(() => undefined)}>CSV</button>
      </div>
      <ErrorBox error={q.error ?? s.error} />
      {days.length > 0 && (
        <table className="table" style={{ marginTop: 8 }}>
          <thead><tr><th>Day</th>{exp.variants.map((v) => <th key={v.key}>{v.key}</th>)}</tr></thead>
          <tbody>{days.map((d) => <tr key={d}><td>{d}</td>{exp.variants.map((v) => <td key={v.key}>{s.data?.assignments.find((a) => a.day === d && a.variant === v.key)?.count ?? 0}</td>)}</tr>)}</tbody>
        </table>
      )}
      <Table<Participant>
        rows={q.data?.items}
        rowKey={(p) => p.identity_id}
        empty="Nobody assigned yet"
        cols={[
          { h: 'Player', c: (p) => <span className="mono">{p.external_id}</span> },
          { h: 'Variant', c: (p) => <code>{p.variant}</code> },
          { h: 'Assigned', c: (p) => fmtDate(p.assigned_at) },
          { h: 'Exposed', c: (p) => p.exposed_at ? `${fmtDate(p.exposed_at)} (${p.exposures}×)` : <span className="muted">not yet</span> },
        ]}
      />
    </div>
  );
}

function ResultsPanel({ exp }: { exp: Experiment }) {
  const [metric, setMetric] = useState('');
  const [applied, setApplied] = useState('');
  const q = useLoad(() => get<Results>(`/experiments/${encodeURIComponent(exp.key)}/results${applied ? '?metric=' + encodeURIComponent(applied) : ''}`), [exp.key, applied, exp.version]);
  const r = q.data;
  const ctrl = r?.variants.find((v) => v.variant === r.control);
  return (
    <div className="card">
      <strong>Results</strong>
      <ErrorBox error={q.error} />
      <div className="row" style={{ marginTop: 10 }}>
        <Field label="Conversion metric (event name)"><input value={metric} onChange={(e) => setMetric(e.target.value)} placeholder="purchase" onKeyDown={(e) => e.key === 'Enter' && setApplied(metric)} /></Field>
        <div className="field fixed"><span className="label">&nbsp;</span><button className="btn ghost" onClick={() => setApplied(metric)}>Apply</button></div>
      </div>
      {r && (
        <>
          <div className="hint" style={{ marginBottom: 8 }}>{fmtNum(r.total_assigned)} identities assigned{r.metric ? ` · metric "${r.metric}"` : ''}
            {r.srm && <> · sample ratio {r.srm.suspicious ? <Badge tone="bad">mismatch (p = {r.srm.p_value.toExponential(1)}) — assignment is skewed, do not trust these results</Badge> : <Badge tone="ok">ok (p = {r.srm.p_value.toFixed(2)})</Badge>}</>}
            <button className="btn ghost sm" style={{ marginLeft: 8 }} onClick={() => download(`/experiments/${encodeURIComponent(exp.key)}/results.csv`, `${exp.key}-results.csv`)}>Download CSV</button>
          </div>

          {r.metrics && r.metrics.length > 0 && (

            <table className="table" style={{ marginBottom: 10 }}>

              <thead><tr><th>Metric</th><th>Kind</th>{r.variants.map((v) => <th key={v.variant}>{v.variant}{r.control === v.variant ? ' (control)' : ''}</th>)}</tr></thead>

              <tbody>{r.metrics.map((m) => <tr key={m.metric}><td><strong>{m.metric}</strong><div className="hint">{m.error ?? `${m.aggregation}`}</div></td><td>{m.kind && <Badge tone={m.kind === 'goal' ? 'ok' : 'info'}>{m.kind}</Badge>}</td>{r.variants.map((v) => { const x = m.variants?.find((y) => y.variant === v.variant); return <td key={v.variant}>{x ? <>{fmtNum(x.value)}{x.lift != null && <span className={'hint ' + (x.lift >= 0 ? 'up' : 'down')}> {x.lift >= 0 ? '+' : ''}{(x.lift * 100).toFixed(1)}%</span>}{x.stats && <div className="hint">{(x.stats.ci_low * 100).toFixed(1)}–{(x.stats.ci_high * 100).toFixed(1)}%{x.stats.prob_beats_control != null && <> · P(beats control) <strong>{(x.stats.prob_beats_control * 100).toFixed(0)}%</strong></>}</div>}</> : '—'}</td>; })}</tr>)}</tbody>

            </table>

          )}
          <table className="table">
            <thead><tr><th>Variant</th><th>Weight</th><th>Assigned</th><th>Exposed</th>{r.metric && <><th>Converted</th><th>Conv. rate</th><th>Lift vs control</th></>}</tr></thead>
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
      title="Experiments"
      path="/experiments"
      blurb="A/B tests with deterministic sticky assignment. Events are tagged with the active variant, so any event name can be used as a conversion metric."
      cols={[
        { h: 'Key', c: (e) => <><strong>{e.key}</strong><div className="hint">{e.name}</div></> },
        { h: 'Status', c: (e) => <Badge tone={TONE[e.status]}>{e.status}</Badge> },
        { h: 'Variants', c: (e) => e.variants.map((v) => v.key).join(' / ') },
        { h: 'Traffic', c: (e) => `${e.percentage}%` },
        { h: 'Flag', c: (e) => e.flag_key ? <code>{e.flag_key}</code> : <span className="muted">—</span> },
        { h: 'Updated', c: (e) => fmtDate(e.updated_at) },
      ]}
      sort={(a, b) => (a.status === 'running' ? -1 : 1) - (b.status === 'running' ? -1 : 1) || a.key.localeCompare(b.key)}
      blank={() => ({ key: '', name: '', description: '', hypothesis: '', condition: null, audiences: [], percentage: 100, control: 'control', flag_key: null, starts_at: null, ends_at: null, tags: [], split_key: 'identity', max_participants: null, enrol_until: null, metrics: [],
        variants: [{ key: 'control', name: 'Control', value: false, weight: 50 }, { key: 'treatment', name: 'Treatment', value: true, weight: 50 }] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} />
          </div>
          <Text label="Hypothesis" value={d.hypothesis} onChange={(hypothesis) => set({ hypothesis })} placeholder="Showing the offer on day 2 increases D7 retention by 3%" />
          <Text label="Description" value={d.description} onChange={(description) => set({ description })} />
          <Field label="Variants" hint="weights are relative; value is what the SDK receives (and what the linked flag resolves to)">
            <VariantsEditor type="json" withName value={(d.variants ?? []) as Variant[]} onChange={(variants) => set({ variants: variants as Variant[] })} />
          </Field>
          <div className="row">
            <Text label="Control variant" value={d.control ?? ''} onChange={(v) => set({ control: v || null })} mono />
            <Text label="Linked flag key" value={d.flag_key ?? ''} onChange={(v) => set({ flag_key: v || null })} mono hint="optional: the flag returns the assigned variant's value" />
            <Num label="Traffic %" value={d.percentage} onChange={(percentage) => set({ percentage: percentage ?? 0 })} min={0} max={100} />
          </div>
          <Field label="Targeting condition"><ConditionEditor optional value={d.condition} onChange={(condition) => set({ condition })} /></Field>
          <Field label="Audiences" hint="identity must belong to all selected"><AudiencePicker value={d.audiences ?? []} onChange={(audiences) => set({ audiences })} /></Field>
          <div className="row">
            <DateTime label="Starts" value={d.starts_at} onChange={(starts_at) => set({ starts_at })} />
            <DateTime label="Ends" value={d.ends_at} onChange={(ends_at) => set({ ends_at })} />
            <DateTime label="Enrolment deadline" value={d.enrol_until ?? null} onChange={(enrol_until) => set({ enrol_until })} />
          </div>
          <div className="row">
            <Num label="Max participants" value={d.max_participants ?? null} onChange={(max_participants) => set({ max_participants: max_participants ?? null })} min={1} hint="blank = unlimited" />
            <Text label="Split key" value={d.split_key ?? 'identity'} onChange={(split_key) => set({ split_key })} mono hint="identity, or property:<path> e.g. property:custom.clan_id" />
          </div>
          <Field label="Metrics" hint="goal / monitor metrics from the catalog, evaluated per variant in results"><MetricsPicker value={d.metrics ?? []} onChange={(metrics) => set({ metrics })} /></Field>
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
      <strong>Phases</strong> <span className="hint">one family: a QA phase on a test audience, then the live phase — copy “as phase” to add the next one; edits to a template apply to the next phase, not the running one</span>
      <ErrorBox error={q.error} />
      <Table<Phase>
        rows={items}
        rowKey={(p) => p.key}
        empty="—"
        cols={[
          { h: '#', w: '50px', c: (p) => p.phase },
          { h: 'Experiment', c: (p) => p.key === exp.key ? <strong>{p.key}</strong> : <a href={`/console/experiments/${encodeURIComponent(p.key)}`}>{p.key}</a> },
          { h: 'Status', c: (p) => <Badge tone={TONE[p.status]}>{p.status}</Badge> },
          { h: 'Audiences', c: (p) => p.audiences.length ? p.audiences.join(', ') : <span className="muted">everyone</span> },
          { h: 'Window', c: (p) => <>{p.starts_at ? fmtDate(p.starts_at) : '—'} → {p.ends_at ? fmtDate(p.ends_at) : '—'}</> },
          { h: 'Winner', c: (p) => p.winner ?? <span className="muted">—</span> },
        ]}
      />
    </div>
  );
}
