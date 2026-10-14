// Baseline: realistic client mix — each VU is one player that authenticates once, then reads
// flags/live events/experiments far more often than it sends events. Ramps to RATE requests/s
// (default 500) and holds for DURATION.
//   ZANSHIN_URL=https://liveops.example.com ZANSHIN_CLIENT_KEY=... k6 run -e RATE=1000 deploy/k6/baseline.js
import { authenticate, flags, liveEvents, experiments, sendEvents } from "./lib.js";

const RATE = Number(__ENV.RATE || 500);
const DURATION = __ENV.DURATION || "2m";

export const options = {
  scenarios: {
    players: {
      executor: "ramping-arrival-rate",
      startRate: Math.max(10, RATE / 10),
      timeUnit: "1s",
      preAllocatedVUs: Math.max(50, RATE / 2),
      maxVUs: RATE * 2,
      stages: [
        { target: RATE, duration: "30s" },
        { target: RATE, duration: DURATION },
        { target: 0, duration: "15s" },
      ],
    },
  },
  thresholds: {
    http_req_failed: ["rate<0.001"],
    "http_req_duration{name:flags}": ["p(95)<50", "p(99)<150"],
    "http_req_duration{name:events}": ["p(95)<50", "p(99)<150"],
    "http_req_duration{name:authenticate}": ["p(95)<150", "p(99)<400"],
  },
  summaryTrendStats: ["avg", "p(50)", "p(95)", "p(99)", "max"],
};

// Module state is per VU in k6, so this is one session per simulated player.
let token = null;

export default function () {
  if (!token) {
    token = authenticate(`k6-${__VU}`);
    return;
  }
  const r = Math.random();
  if (r < 0.55) flags(token);
  else if (r < 0.7) liveEvents(token);
  else if (r < 0.8) experiments(token);
  else sendEvents(token, 1 + Math.floor(Math.random() * 4));
}
