// Smoke: one player, full client loop, strict thresholds. `k6 run deploy/k6/smoke.js`
import { sleep } from "k6";
import { authenticate, flags, liveEvents, experiments, sendEvents } from "./lib.js";

export const options = {
  vus: 1,
  iterations: 20,
  thresholds: {
    checks: ["rate==1"],
    http_req_failed: ["rate==0"],
    "http_req_duration{name:flags}": ["p(95)<100"],
    "http_req_duration{name:events}": ["p(95)<100"],
  },
};

export default function () {
  const token = authenticate(`k6-smoke-${__VU}`);
  flags(token);
  liveEvents(token);
  experiments(token);
  sendEvents(token);
  sleep(0.2);
}
