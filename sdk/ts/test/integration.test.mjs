// Runs against a live server when ZANSHIN_TEST_URL and ZANSHIN_TEST_CLIENT_KEY are set.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import WebSocket from 'ws';
import { ZanshinClient, ZanshinError } from '../dist/index.js';

const url = process.env.ZANSHIN_TEST_URL;
const apiKey = process.env.ZANSHIN_TEST_CLIENT_KEY;
const adminKey = process.env.ZANSHIN_TEST_ADMIN_KEY;

test('client flow against a live server', { skip: !url || !apiKey }, async () => {
  const sessions = [];
  const z = new ZanshinClient({ url, apiKey, onSession: (s) => sessions.push(s) });
  const id = 'ts-sdk-' + Date.now();
  const s = await z.authenticate(id, { default: { platform: 'node' }, custom: { nick: 'Neo' } });
  assert.equal(s.identity.external_id, id);
  assert.ok(z.isAuthenticated);
  assert.equal(sessions.length, 1);

  const props = await z.updateProperties({ custom: { level: 3 } });
  assert.equal(props.custom.level, 3);
  assert.equal(props.custom.nick, 'Neo');

  const flags = await z.flags();
  assert.ok(Array.isArray(flags));
  await assert.rejects(z.flag('definitely_missing_flag_ts'), (e) => e instanceof ZanshinError && e.isNotFound);
  assert.equal(await z.flagValue('definitely_missing_flag_ts', 'fallback'), 'fallback');
  assert.ok(Array.isArray(await z.audiences()));
  assert.ok(Array.isArray(await z.experiments()));
  assert.ok(Array.isArray(await z.liveEvents({ upcoming: true })));
  assert.ok(Array.isArray(await z.inbox()));
  assert.equal(await z.track({ name: 'ts.ping', value: '1', metadata: { k: 'v' } }, { name: 'ts.pong' }), 2);
  assert.equal(await z.event('ts.level_up', 7), 1);
  await assert.rejects(z.track({ name: '$reserved' }), (e) => e instanceof ZanshinError && e.status === 400);

  // Refresh rotates both tokens and notifies.
  const before = z.token;
  await z.refresh();
  assert.notEqual(z.token, before);
  assert.equal(sessions.length, 2);

  // Stream: hello, then live updates when an admin changes a flag.
  const stream = z.stream({ webSocket: (u) => new WebSocket(u) });
  const got = [];
  stream.onMessage((m) => got.push(m));
  const opened = new Promise((resolve) => stream.onMessage((m) => m.type === 'hello' && resolve()));
  stream.connect();
  await Promise.race([opened, new Promise((_, rej) => setTimeout(() => rej(new Error('no hello')), 5000))]);
  assert.ok(stream.connected);

  if (adminKey) {
    const key = 'ts_sdk_' + Date.now();
    const flagEvent = new Promise((resolve) => stream.onFlag((f) => f.key === key && resolve(f)));
    const res = await fetch(`${url}/admin/v1/flags`, {
      method: 'POST',
      headers: { 'x-api-key': adminKey, 'content-type': 'application/json' },
      body: JSON.stringify({ key, value_type: 'bool', default_value: true }),
    });
    assert.equal(res.status, 201);
    const f = await Promise.race([flagEvent, new Promise((_, rej) => setTimeout(() => rej(new Error('no flag push')), 5000))]);
    assert.equal(f.value, true);
    assert.equal(stream.flags.get(key)?.value, true);
    const deleted = new Promise((resolve) => stream.onFlagDeleted((k) => k === key && resolve(k)));
    await fetch(`${url}/admin/v1/flags/${key}`, { method: 'DELETE', headers: { 'x-api-key': adminKey } });
    await Promise.race([deleted, new Promise((_, rej) => setTimeout(() => rej(new Error('no delete push')), 5000))]);
    assert.ok(!stream.flags.has(key));
  }
  stream.close();

  await z.logout();
  assert.ok(!z.isAuthenticated);
  assert.equal(sessions.at(-1), null);
  await assert.rejects(z.flags(), (e) => e instanceof ZanshinError && e.isUnauthenticated);
});
