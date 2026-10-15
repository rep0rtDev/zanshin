import test from 'node:test';
import assert from 'node:assert/strict';
import { ZanshinProvider } from '../dist/openfeature.js';

const bundle = { generated_at: '', managed_audiences: [], audiences: [], flags: [
  { key: 'shop', value_type: 'bool', default_value: false, enabled: true, salt: 's', version: 1, rules: [{ id: 'r', condition: { prop: 'custom.level', op: 'gte', value: 10 }, percentage: 100, value: true }] },
  { key: 'title', value_type: 'string', default_value: 'hi', enabled: true, salt: 's', version: 1, rules: [] },
] };
const fakeClient = { rules: async () => bundle, serverFlags: async () => [{ key: 'shop', value: true, value_type: 'bool', reason: { kind: 'override' }, version: 2 }] };

test('openfeature provider, local mode', async () => {
  const p = new ZanshinProvider({ client: fakeClient, local: true });
  await p.initialize();
  let d = await p.resolveBooleanEvaluation('shop', false, { targetingKey: 'u1', level: 12 });
  assert.equal(d.value, true); assert.equal(d.reason, 'TARGETING_MATCH'); assert.equal(d.flagMetadata.rule_id, 'r');
  d = await p.resolveBooleanEvaluation('shop', false, { targetingKey: 'u1', custom: { level: 3 } });
  assert.equal(d.value, false); assert.equal(d.reason, 'DEFAULT');
  d = await p.resolveStringEvaluation('shop', 'x', { targetingKey: 'u1' });
  assert.equal(d.errorCode, 'TYPE_MISMATCH'); assert.equal(d.value, 'x');
  d = await p.resolveNumberEvaluation('nope', 7, { targetingKey: 'u1' });
  assert.equal(d.errorCode, 'FLAG_NOT_FOUND'); assert.equal(d.value, 7);
  assert.equal((await p.resolveStringEvaluation('title', 'x', {})).value, 'hi');
});

test('openfeature provider, remote mode', async () => {
  const p = new ZanshinProvider({ client: fakeClient });
  const d = await p.resolveBooleanEvaluation('shop', false, { targetingKey: 'u1' });
  assert.equal(d.value, true); assert.equal(d.reason, 'TARGETING_MATCH');
  assert.equal((await p.resolveBooleanEvaluation('shop', false, {})).errorCode, 'GENERAL');
});
