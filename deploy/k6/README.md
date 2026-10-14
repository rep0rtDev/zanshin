# Load tests (k6)

Scripts target a running server with a **client** API key:

```sh
export ZANSHIN_URL=http://localhost:7550 ZANSHIN_CLIENT_KEY=<client key>
k6 run deploy/k6/smoke.js                    # 1 VU, strict thresholds — run after every deploy
k6 run -e RATE=500 deploy/k6/baseline.js     # request mix at 500 req/s for 2 minutes
k6 run -e STREAMS=1000 deploy/k6/stream.js   # 1000 WebSocket streams held for 60 s
```

`baseline.js` mix: 55 % `GET /v1/flags`, 15 % live events, 10 % experiments, 20 % `POST /v1/events`
(1–4 events), plus one `authenticate` per simulated player (VU). Thresholds encode the
service level the production docs promise (p95 < 50 ms, p99 < 150 ms for reads and ingest,
error rate < 0.1 %); tune `RATE` up until a threshold breaks to find the ceiling of a deployment.

See [docs/production.md](../../docs/production.md#capacity-and-baselines) for reference numbers.
