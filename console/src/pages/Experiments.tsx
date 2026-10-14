import { useState } from 'react';
import { get, post, useLoad } from '../api';
import { EntityPage, Text, Num, Tags, DateTime, CONDITION_HINT } from '../crud';
import { Badge, ErrorBox, Field, JsonField, fmtDate, fmtNum } from '../ui';

type Variant = { key: string; name: string; value: unknown; weight: number };
type Experiment = {
  key: string; name: string; description: string; hypothesis: string; status: 'draft' | 'running' | 'paused' | 'stopped';
  condition: unknown; audiences: string[]; percentage: number; variants: Variant[]; control: string | null; flag_key: string | null;
  starts_at: string | null; ends_at: string | null; tags: string[]; version: number; updated_at: string;
};
type Results = {
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
          <div className="hint" style={{ marginBottom: 8 }}>{fmtNum(r.total_assigned)} identities assigned{r.metric ? ` · metric "${r.metric}"` : ''}</div>
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
      blank={() => ({ key: '', name: '', description: '', hypothesis: '', condition: null, audiences: [], percentage: 100, control: 'control', flag_key: null, starts_at: null, ends_at: null, tags: [],
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
            <JsonField value={d.variants} onChange={(v) => set({ variants: (v as Variant[]) ?? [] })} rows={6} />
          </Field>
          <div className="row">
            <Text label="Control variant" value={d.control ?? ''} onChange={(v) => set({ control: v || null })} mono />
            <Text label="Linked flag key" value={d.flag_key ?? ''} onChange={(v) => set({ flag_key: v || null })} mono hint="optional: the flag returns the assigned variant's value" />
            <Num label="Traffic %" value={d.percentage} onChange={(percentage) => set({ percentage: percentage ?? 0 })} min={0} max={100} />
          </div>
          <Field label="Targeting condition" hint={CONDITION_HINT}><JsonField value={d.condition ?? undefined} onChange={(condition) => set({ condition: condition ?? null })} rows={4} /></Field>
          <Text label="Audiences" value={(d.audiences ?? []).join(', ')} onChange={(v) => set({ audiences: v.split(',').map((s) => s.trim()).filter(Boolean) })} hint="audience keys, comma-separated; identity must be in all of them" />
          <div className="row">
            <DateTime label="Starts" value={d.starts_at} onChange={(starts_at) => set({ starts_at })} />
            <DateTime label="Ends" value={d.ends_at} onChange={(ends_at) => set({ ends_at })} />
          </div>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(e, reload) => (<><Lifecycle exp={e} reload={reload} /><ResultsPanel exp={e} /></>)}
    />
  );
}
