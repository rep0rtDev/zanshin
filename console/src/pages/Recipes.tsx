import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { ApiError, post } from '../api';
import { toast } from '../toast';
import { Badge, ErrorBox, Field, Page } from '../ui';

type Param = { key: string; label: string; type: 'text' | 'number'; value: string; hint?: string };
type Step = { kind: 'flag' | 'live-event' | 'message' | 'experiment' | 'audience'; path: string; body: Record<string, unknown> };
type Recipe = {
  id: string;
  name: string;
  tagline: string;
  description: string;
  params: Param[];
  build: (p: Record<string, string>) => Step[];
};

const num = (s: string, d: number) => (Number.isFinite(Number(s)) && s !== '' ? Number(s) : d);
const iso = (offsetMin: number) => new Date(Date.now() + offsetMin * 60_000).toISOString();

const RECIPES: Recipe[] = [
  {
    id: 'timed_offer',
    name: 'Timed offer',
    tagline: 'Limited-time bundle shown to lapsed players',
    description: 'A live event opens a discounted bundle for a few hours. The flag carries the offer payload the client renders; the event overrides it while running; an inbox message announces it.',
    params: [
      { key: 'key', label: 'Key prefix', type: 'text', value: 'starter_offer' },
      { key: 'hours', label: 'Duration (hours)', type: 'number', value: '6' },
      { key: 'price', label: 'Price (USD)', type: 'number', value: '4.99' },
      { key: 'discount', label: 'Discount %', type: 'number', value: '40' },
    ],
    build: (p) => [
      { kind: 'flag', path: '/flags', body: { key: `${p.key}_offer`, name: 'Timed offer', value_type: 'json', enabled: true, default_value: { active: false }, tags: ['recipe', 'offer'] } },
      { kind: 'live-event', path: '/live-events', body: { key: `${p.key}_window`, name: 'Timed offer window', starts_at: iso(1), duration_secs: num(p.hours, 6) * 3600, payload: { price: num(p.price, 4.99), discount: num(p.discount, 40) }, flag_overrides: { [`${p.key}_offer`]: { active: true, price: num(p.price, 4.99), discount: num(p.discount, 40) } }, tags: ['recipe'] } },
      { kind: 'message', path: '/messages', body: { key: `${p.key}_announce`, name: 'Timed offer announcement', title: `${num(p.discount, 40)}% off — ${num(p.hours, 6)} hours only`, body: 'A special bundle is waiting in the shop.', enabled: true, schedule: { kind: 'live_event', live_event: `${p.key}_window` }, payload: { screen: 'shop', offer: `${p.key}_offer` }, tags: ['recipe'] } },
    ],
  },
  {
    id: 'dynamic_pricing',
    name: 'Dynamic pricing test',
    tagline: 'A/B three price points on one SKU',
    description: 'One flag holds the price; an experiment splits players over three variants and tracks revenue. Interrupt the experiment with a winner to roll the price out to everyone.',
    params: [
      { key: 'key', label: 'Key prefix', type: 'text', value: 'gems_100' },
      { key: 'base', label: 'Control price', type: 'number', value: '1.99' },
      { key: 'low', label: 'Low price', type: 'number', value: '0.99' },
      { key: 'high', label: 'High price', type: 'number', value: '2.99' },
    ],
    build: (p) => [
      { kind: 'flag', path: '/flags', body: { key: `${p.key}_price`, name: 'SKU price', value_type: 'number', enabled: true, default_value: num(p.base, 1.99), tags: ['recipe', 'pricing'] } },
      { kind: 'experiment', path: '/experiments', body: { key: `${p.key}_price_test`, name: 'Price test', status: 'running', flag_key: `${p.key}_price`, control: 'control', variants: [{ key: 'control', value: num(p.base, 1.99), weight: 1 }, { key: 'low', value: num(p.low, 0.99), weight: 1 }, { key: 'high', value: num(p.high, 2.99), weight: 1 }], tags: ['recipe'] } },
    ],
  },
  {
    id: 'offer_wall',
    name: 'Offer wall',
    tagline: 'Personalised shop tab by spender segment',
    description: 'Two audiences (non-payers and payers) and a JSON flag whose rules serve a different offer list to each. Edit the rules later to retarget without a client release.',
    params: [
      { key: 'key', label: 'Key prefix', type: 'text', value: 'offer_wall' },
      { key: 'prop', label: 'Spend property', type: 'text', value: 'custom.total_spend', hint: 'numeric custom property your game sets' },
    ],
    build: (p) => [
      { kind: 'audience', path: '/audiences', body: { key: `${p.key}_non_payers`, name: 'Non-payers', condition: { any: [{ prop: p.prop, op: 'eq', value: 0 }, { prop: p.prop, op: 'not_exists' }] }, tags: ['recipe'] } },
      { kind: 'audience', path: '/audiences', body: { key: `${p.key}_payers`, name: 'Payers', condition: { prop: p.prop, op: 'gt', value: 0 }, tags: ['recipe'] } },
      { kind: 'flag', path: '/flags', body: { key: `${p.key}`, name: 'Offer wall', value_type: 'json', enabled: true, default_value: { offers: ['starter_pack'] }, rules: [
        { id: 'payers', name: 'Payers', audiences: [`${p.key}_payers`], percentage: 100, value: { offers: ['whale_bundle', 'gem_pile', 'season_pass'] } },
        { id: 'non_payers', name: 'Non-payers', audiences: [`${p.key}_non_payers`], percentage: 100, value: { offers: ['starter_pack', 'first_purchase_bonus'] } },
      ], tags: ['recipe', 'offer'] } },
    ],
  },
  {
    id: 'return_incentive',
    name: 'Return incentive',
    tagline: 'Welcome-back reward for players away 7+ days',
    description: 'An audience of players not seen for a week and a campaign that drops a reward in their inbox once. The payload tells the client what to grant.',
    params: [
      { key: 'key', label: 'Key prefix', type: 'text', value: 'welcome_back' },
      { key: 'days', label: 'Away for (days)', type: 'number', value: '7' },
      { key: 'gold', label: 'Reward gold', type: 'number', value: '500' },
    ],
    build: (p) => [
      { kind: 'audience', path: '/audiences', body: { key: `${p.key}_lapsed`, name: 'Lapsed players', condition: { prop: 'computed.last_seen_at', op: 'older_than', value: `${num(p.days, 7)}d` }, tags: ['recipe'] } },
      { kind: 'message', path: '/messages', body: { key: `${p.key}_reward`, name: 'Welcome back reward', title: 'Welcome back!', body: `Here are ${num(p.gold, 500)} gold to get you going again.`, enabled: true, audiences: [`${p.key}_lapsed`], payload: { grant: { gold: num(p.gold, 500) } }, ttl_secs: 7 * 86400, tags: ['recipe'] } },
    ],
  },
  {
    id: 'soft_launch',
    name: 'Soft launch gate',
    tagline: 'Feature on for a country and a % of everyone else',
    description: 'A bool flag with two rules: fully on for a launch country, a small percentage elsewhere. Move the slider in the flag editor as confidence grows.',
    params: [
      { key: 'key', label: 'Flag key', type: 'text', value: 'new_mode' },
      { key: 'country', label: 'Launch country (ISO)', type: 'text', value: 'NZ' },
      { key: 'pct', label: 'Rollout elsewhere %', type: 'number', value: '5' },
    ],
    build: (p) => [
      { kind: 'flag', path: '/flags', body: { key: p.key, name: 'Soft launch gate', value_type: 'bool', enabled: true, default_value: false, rules: [
        { id: 'launch_country', name: `${p.country} launch`, condition: { prop: 'default.country', op: 'eq', value: p.country.toUpperCase() }, percentage: 100, value: true },
        { id: 'global_rollout', name: 'Gradual rollout', percentage: num(p.pct, 5), value: true },
      ], tags: ['recipe', 'rollout'] } },
    ],
  },
  {
    id: 'holiday_event',
    name: 'Holiday event',
    tagline: 'Weekend-recurring event with doubled rewards',
    description: 'A recurring live event (every Saturday 00:00 UTC for 48 h) that overrides the reward multiplier flag while running and posts a reminder to the inbox at each start.',
    params: [
      { key: 'key', label: 'Key prefix', type: 'text', value: 'weekend_bonus' },
      { key: 'mult', label: 'Reward multiplier', type: 'number', value: '2' },
    ],
    build: (p) => [
      { kind: 'flag', path: '/flags', body: { key: `${p.key}_multiplier`, name: 'Reward multiplier', value_type: 'number', enabled: true, default_value: 1, tags: ['recipe', 'event'] } },
      { kind: 'live-event', path: '/live-events', body: { key: `${p.key}`, name: 'Weekend bonus', starts_at: iso(1), duration_secs: 48 * 3600, recurrence: '0 0 0 * * 6', payload: { multiplier: num(p.mult, 2) }, flag_overrides: { [`${p.key}_multiplier`]: num(p.mult, 2) }, tags: ['recipe'] } },
      { kind: 'message', path: '/messages', body: { key: `${p.key}_reminder`, name: 'Weekend bonus reminder', title: `${num(p.mult, 2)}x rewards all weekend`, body: 'Play now — every reward is multiplied until Monday.', enabled: true, schedule: { kind: 'live_event', live_event: `${p.key}` }, tags: ['recipe'] } },
    ],
  },
];

type Result = { step: Step; status: 'created' | 'exists' | 'failed'; detail?: string };

export default function Recipes() {
  const nav = useNavigate();
  const [open, setOpen] = useState<string | null>(null);
  const [params, setParams] = useState<Record<string, Record<string, string>>>({});
  const [results, setResults] = useState<Record<string, Result[]>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<Error | null>(null);

  const valuesFor = (r: Recipe) => Object.fromEntries(r.params.map((p) => [p.key, params[r.id]?.[p.key] ?? p.value]));

  const apply = async (r: Recipe) => {
    setBusy(r.id);
    setErr(null);
    const out: Result[] = [];
    for (const step of r.build(valuesFor(r))) {
      try {
        await post(step.path, step.body);
        out.push({ step, status: 'created' });
      } catch (e) {
        if (e instanceof ApiError && e.status === 409) out.push({ step, status: 'exists' });
        else {
          out.push({ step, status: 'failed', detail: (e as Error).message });
          setErr(e as Error);
          break;
        }
      }
      setResults((s) => ({ ...s, [r.id]: [...out] }));
    }
    setBusy(null);
    if (out.every((o) => o.status !== 'failed')) toast(`${r.name}: ${out.filter((o) => o.status === 'created').length} objects created`);
  };

  return (
    <Page title="Recipes">
      <p className="hint">Ready-made LiveOps setups. Each recipe creates the flags, audiences, live events, campaigns and experiments behind a common mechanic with your keys and numbers — then edit them like anything else. Nothing is created until you press Create.</p>
      <ErrorBox error={err} />
      <div className="grid c2">
        {RECIPES.map((r) => {
          const vals = valuesFor(r);
          const steps = r.build(vals);
          const res = results[r.id];
          return (
            <div className="card" key={r.id}>
              <div className="row" style={{ justifyContent: 'space-between', alignItems: 'baseline' }}>
                <div>
                  <strong>{r.name}</strong>
                  <div className="hint">{r.tagline}</div>
                </div>
                <button className="btn" onClick={() => setOpen(open === r.id ? null : r.id)}>{open === r.id ? 'Close' : 'Configure'}</button>
              </div>
              <p className="hint" style={{ marginTop: 8 }}>{r.description}</p>
              <div style={{ marginTop: 6 }}>
                {steps.map((s, i) => <Badge key={i} tone="info">{s.kind} · {String(s.body.key)}</Badge>)}
              </div>
              {open === r.id && (
                <div className="stack" style={{ marginTop: 12 }}>
                  <div className="row">
                    {r.params.map((p) => (
                      <Field key={p.key} label={p.label} hint={p.hint}>
                        <input type={p.type} value={vals[p.key]} onChange={(e) => setParams((s) => ({ ...s, [r.id]: { ...(s[r.id] ?? {}), [p.key]: e.target.value } }))} />
                      </Field>
                    ))}
                  </div>
                  <details>
                    <summary className="hint">Objects that will be created ({steps.length})</summary>
                    <pre style={{ whiteSpace: 'pre-wrap', fontSize: 12 }}>{JSON.stringify(steps.map((s) => ({ [s.kind]: s.body })), null, 2)}</pre>
                  </details>
                  <div className="row">
                    <button className="btn primary" disabled={busy === r.id} onClick={() => apply(r)}>{busy === r.id ? 'Creating…' : 'Create'}</button>
                  </div>
                  {res && (
                    <ul className="plain">
                      {res.map((o, i) => (
                        <li key={i}>
                          <Badge tone={o.status === 'created' ? 'ok' : o.status === 'exists' ? 'warn' : 'bad'}>{o.status}</Badge>{' '}
                          <a onClick={() => nav(`${o.step.path === '/live-events' ? '/live-events' : o.step.path}/${encodeURIComponent(String(o.step.body.key))}`)} style={{ cursor: 'pointer' }}>
                            {o.step.kind} <code>{String(o.step.body.key)}</code>
                          </a>
                          {o.detail && <span className="hint"> — {o.detail}</span>}
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </Page>
  );
}
