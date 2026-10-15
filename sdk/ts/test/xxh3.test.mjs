import test from 'node:test';
import assert from 'node:assert/strict';
import { bucket, weightedPick, inRollout } from '../dist/xxh3.js';

// vectors from the server's zanshin_core::hash::bucket
test('bucket matches the server', () => {
  assert.equal(bucket('flag:a', 's1', 'user-1'), 1931);
  assert.equal(bucket('audience:vip', '', '00000000-0000-0000-0000-000000000001'), 6244);
  assert.equal(bucket('flag:ünïcode', 'sålt', 'пользователь-42'), 1054);
  assert.equal(bucket('flag:long', 'salt', 'x'.repeat(300)), 9691);
  assert.equal(bucket('', '', ''), 213);
  assert.equal(weightedPick('flag:a', 's1:r1:variant', 'user-1', [1, 1, 2]), 0);
  assert.equal(inRollout('flag:a', 's1:r1', 'user-1', 50), false);
});
