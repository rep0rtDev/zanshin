import test from 'node:test';
import assert from 'node:assert/strict';
import { LocalEvaluator, flatten, matches } from '../dist/local.js';

const bundle = {
  generated_at: '2026-01-01T00:00:00Z',
  flags: [
    { key: 'shop', value_type: 'bool', default_value: false, enabled: true, salt: 's', version: 3, rules: [
      { id: 'vip', condition: { all: [] }, audiences: ['vip'], percentage: 100, value: true },
      { id: 'de', condition: { prop: 'default.country', op: 'in', value: ['DE', 'AT'] }, percentage: 100, variants: [{ key: 'a', value: true, weight: 1 }, { key: 'b', value: false, weight: 1 }] },
    ] },
    { key: 'off', value_type: 'string', default_value: 'x', enabled: false, salt: 's', version: 1, rules: [] },
    { key: 'child', value_type: 'number', default_value: 1, enabled: true, salt: 's', version: 1, prerequisites: [{ flag: 'shop', value: true }],
      rules: [{ id: 'r', condition: { prop: 'custom.level', op: 'gte', value: '10' }, percentage: 100, value: 2 }] },
  ],
  audiences: [{ key: 'vip', condition: { prop: 'custom.tags', op: 'contains', value: 'vip' }, percentage: 100, salt: '', enabled: true }],
  managed_audiences: ['whales'],
};

test('local evaluation mirrors the server', () => {
  const e = new LocalEvaluator(bundle);
  const vip = flatten('u1', { default: { country: 'FR' }, custom: { tags: ['vip'], level: 12 } });
  let r = e.evaluate('shop', 'u1', vip);
  assert.equal(r.value, true); assert.deepEqual(r.reason, { kind: 'rule', rule_id: 'vip' });
  r = e.evaluate('shop', 'u1', flatten('u1', { default: { country: 'DE' } }));
  assert.equal(r.reason.kind, 'variant'); assert.equal(r.reason.rule_id, 'de'); assert.ok(r.variant);
  r = e.evaluate('shop', 'u1', flatten('u1'));
  assert.equal(r.reason.kind, 'default'); assert.equal(r.value, false);
  assert.equal(e.evaluate('off', 'u1', vip).reason.kind, 'disabled');
  r = e.evaluate('child', 'u1', vip);
  assert.equal(r.value, 2); assert.equal(r.reason.kind, 'rule');
  r = e.evaluate('child', 'u1', flatten('u1', { custom: { level: 12 } }));
  assert.deepEqual(r.reason, { kind: 'prerequisite', flag: 'shop' }); assert.equal(r.value, 1); assert.equal('variant' in r, false);
  assert.equal(e.evaluate('nope', 'u1', vip), undefined);
  assert.deepEqual(e.evaluateAll('u1', vip).map((f) => f.key), ['child', 'off', 'shop']);
  assert.equal(e.dependsOnManaged('shop'), false);
});

test('conditions', () => {
  const p = { 'default.country': 'DE', 'default.app_version': '1.4.2', 'custom.level': 12, 'custom.tags': ['beta', 'vip'], 'custom.beta': 'true', 'default.created_at': '2020-01-01T00:00:00Z' };
  for (const c of [
    { prop: 'default.country', op: 'eq', value: 'DE' }, { prop: 'default.country', op: 'in', value: ['AT', 'DE'] },
    { prop: 'custom.level', op: 'gte', value: 12 }, { prop: 'custom.level', op: 'lt', value: '20' },
    { prop: 'custom.tags', op: 'contains', value: 'vip' }, { prop: 'custom.beta', op: 'eq', value: true },
    { prop: 'default.app_version', op: 'semver_gte', value: 'v1.4' }, { prop: 'default.app_version', op: 'semver_lt', value: '2.0.0' },
    { prop: 'default.country', op: 'matches', value: '^D' }, { prop: 'missing', op: 'not_exists' }, { prop: 'missing', op: 'neq', value: 1 },
    { prop: 'default.created_at', op: 'older_than', value: 86400 }, { not: { prop: 'default.country', op: 'eq', value: 'FR' } },
    { any: [{ prop: 'default.country', op: 'eq', value: 'FR' }, { prop: 'custom.level', op: 'gt', value: 10 }] },
  ]) assert.ok(matches(c, p), JSON.stringify(c));
  for (const c of [
    { prop: 'default.country', op: 'not_in', value: ['AT', 'DE'] }, { prop: 'custom.level', op: 'gt', value: 12 },
    { prop: 'missing', op: 'eq', value: 1 }, { prop: 'default.created_at', op: 'within_last', value: 60 },
  ]) assert.ok(!matches(c, p), JSON.stringify(c));
});
