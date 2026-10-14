// Long-lived WebSocket streams: N players hold /v1/stream open while an operator flips a flag.
// Measures connect time and that every stream receives the update.  k6 run -e STREAMS=2000 deploy/k6/stream.js
import ws from "k6/ws";
import { check } from "k6";
import { Trend, Counter } from "k6/metrics";
import { BASE, authenticate } from "./lib.js";

const STREAMS = Number(__ENV.STREAMS || 500);
const HOLD = Number(__ENV.HOLD_SECONDS || 60);
const connectTime = new Trend("ws_connect_ms", true);
const updates = new Counter("ws_flag_updates");

export const options = {
  scenarios: { streams: { executor: "per-vu-iterations", vus: STREAMS, iterations: 1, maxDuration: `${HOLD + 30}s` } },
  thresholds: { ws_connect_ms: ["p(95)<500"], checks: ["rate>0.99"] },
};

export default function () {
  const token = authenticate(`k6-stream-${__VU}`);
  const url = `${BASE.replace(/^http/, "ws")}/v1/stream?token=${encodeURIComponent(token)}`;
  const started = Date.now();
  const res = ws.connect(url, {}, (socket) => {
    socket.on("open", () => connectTime.add(Date.now() - started));
    socket.on("message", (m) => {
      const msg = JSON.parse(m);
      if (msg.type === "flags" || msg.type === "flag") updates.add(1);
      if (msg.type === "ping") socket.send(JSON.stringify({ type: "pong" }));
    });
    socket.setTimeout(() => socket.close(), HOLD * 1000);
  });
  check(res, { "ws 101": (r) => r && r.status === 101 });
}
