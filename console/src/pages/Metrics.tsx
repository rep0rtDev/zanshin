import { get, useLoad } from '../api';
import { EntityPage, Tags, Text } from '../crud';
import { Badge, ErrorBox, Field, fmtDate } from '../ui';

type Metric = {
  id: string; key: string; name: string; description: string; kind: 'goal' | 'monitor'; event_name: string;
  aggregation: 'count' | 'unique' | 'sum' | 'avg' | 'conversion'; field: string; tags: string[]; version: number; updated_at: string;
};

const AGG: Record<Metric['aggregation'], string> = {
  count: 'count — number of events', unique: 'unique — distinct players', sum: 'sum — of a numeric field',
  avg: 'avg — sum / count', conversion: 'conversion — players with the event / players assigned',
};

export default function Metrics() {
  return (
    <EntityPage<Metric>
      title="Metrics"
      path="/metrics"
      blurb="Named measurements over events. Attach them to experiments as goal (success) or monitor (guardrail) metrics; results report each per variant with lift vs control."
      cols={[
        { h: 'Key', c: (m) => <><strong>{m.key}</strong><div className="hint">{m.name}</div></> },
        { h: 'Kind', c: (m) => <Badge tone={m.kind === 'goal' ? 'ok' : 'info'}>{m.kind}</Badge> },
        { h: 'Event', c: (m) => <code>{m.event_name}</code> },
        { h: 'Aggregation', c: (m) => <>{m.aggregation}{m.field ? <span className="hint"> of {m.field}</span> : null}</> },
        { h: 'Updated', c: (m) => fmtDate(m.updated_at) },
      ]}
      blank={() => ({ key: '', name: '', description: '', kind: 'goal', event_name: '', aggregation: 'count', field: '', tags: [] })}
      form={(d, set, isNew) => (
        <>
          <div className="row">
            <Text label="Key" value={d.key} onChange={(key) => set({ key })} disabled={!isNew} mono />
            <Text label="Name" value={d.name} onChange={(name) => set({ name })} />
          </div>
          <Text label="Description" value={d.description} onChange={(description) => set({ description })} />
          <div className="row">
            <Field label="Kind" hint="goal = what the experiment should move; monitor = guardrail">
              <select value={d.kind} onChange={(e) => set({ kind: e.target.value as Metric['kind'] })}><option value="goal">goal</option><option value="monitor">monitor</option></select>
            </Field>
            <Text label="Event name" value={d.event_name} onChange={(event_name) => set({ event_name })} mono placeholder="purchase" />
          </div>
          <div className="row">
            <Field label="Aggregation">
              <select value={d.aggregation} onChange={(e) => set({ aggregation: e.target.value as Metric['aggregation'] })}>{(Object.keys(AGG) as Metric['aggregation'][]).map((k) => <option key={k} value={k}>{AGG[k]}</option>)}</select>
            </Field>
            {(d.aggregation === 'sum' || d.aggregation === 'avg') && <Text label="Field" value={d.field} onChange={(field) => set({ field })} mono placeholder="value (default) or a metadata key, e.g. usd" />}
          </div>
          <Tags value={d.tags} onChange={(tags) => set({ tags })} />
        </>
      )}
      extra={(m) => <UsedBy metric={m} />}
    />
  );
}

function UsedBy({ metric }: { metric: Metric }) {
  const q = useLoad(() => get<{ used_by: string[] }>(`/metrics/${encodeURIComponent(metric.key)}`), [metric.key]);
  return (
    <div className="card">
      <strong>Used by</strong>
      <ErrorBox error={q.error} />
      <div className="hint" style={{ marginTop: 6 }}>{q.data ? (q.data.used_by.length ? q.data.used_by.map((k) => <code key={k} style={{ marginRight: 8 }}>{k}</code>) : 'No experiment uses this metric yet') : '…'}</div>
    </div>
  );
}
