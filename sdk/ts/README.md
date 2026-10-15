# @zanshin/sdk

TypeScript client for Zanshin (browser and Node ≥ 18, zero dependencies).

```ts
import { ZanshinClient } from '@zanshin/sdk';

const z = new ZanshinClient({
  url: 'https://liveops.example.com',
  apiKey: CLIENT_KEY,
  onSession: (s) => (s ? localStorage.setItem('zanshin', JSON.stringify(s)) : localStorage.removeItem('zanshin')),
});

await z.authenticate(deviceId, { default: { platform: 'web', version: '1.4.2' } });
// or: await z.restore(saved.token, saved.refreshToken);

const shopV2 = await z.flagValue('shop_v2', false);
const experiments = await z.experiments();      // sticky variants
await z.expose('price_test');

// link an anonymous identity to the signed-in account (merges if it exists) — the client switches session
await z.identify('account-42');
// explicit live-event participation and inbox housekeeping
const ev = await z.joinLiveEvent('weekend_boost');
await z.leaveLiveEvent('weekend_boost');
await z.deleteInboxMessage(messageId);
const events = await z.liveEvents();            // active now
const inbox = await z.inbox();                  // delivered campaigns + direct messages
await z.event('purchase', 9.99, { sku: 'gold' });

const stream = z.stream();                      // live updates over WebSocket
stream.onFlag((f) => console.log('flag changed', f.key, f.value));
stream.onInbox((msgs) => render(msgs));
stream.connect();
```

Access tokens are refreshed automatically (shared single refresh on concurrent calls); the
stream reconnects with backoff and re-authenticates after refresh. In Node < 22 pass a WebSocket
implementation: `z.stream({ webSocket: (u) => new WebSocket(u) })` with the `ws` package.

## Local evaluation and OpenFeature (server side)

```ts
import { ZanshinClient, LocalEvaluator, flatten, ZanshinProvider } from '@zanshin/sdk';
const z = new ZanshinClient({ url, apiKey: serverKey });
const local = new LocalEvaluator(await z.rules());          // GET /server/v1/rules
local.evaluate('shop', 'player-42', flatten('player-42', { custom: { level: 7 } }));

OpenFeature.setProvider(new ZanshinProvider({ client: z, local: true }));
await OpenFeature.getClient().getBooleanValue('shop', false, { targetingKey: 'player-42', level: 7 });
```

Same hashing, operators and reasons as the server; overrides, experiments and live events are not
applied locally and managed audiences never match (`dependsOnManaged(key)`).
