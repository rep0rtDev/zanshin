import http from "k6/http";
import { check } from "k6";

export const BASE = __ENV.ZANSHIN_URL || "http://localhost:7550";
export const CLIENT_KEY = __ENV.ZANSHIN_CLIENT_KEY || "";
export const headers = (token) => ({
  "Content-Type": "application/json",
  "x-api-key": CLIENT_KEY,
  ...(token ? { Authorization: `Bearer ${token}` } : {}),
});

export function authenticate(externalId) {
  const res = http.post(
    `${BASE}/v1/authenticate`,
    JSON.stringify({ id: externalId, properties: { custom: { platform: "k6", level: Math.floor(Math.random() * 60) } } }),
    { headers: headers(), tags: { name: "authenticate" } },
  );
  check(res, { "authenticate 200": (r) => r.status === 200 });
  return res.json("token");
}

export function flags(token) {
  const res = http.get(`${BASE}/v1/flags`, { headers: headers(token), tags: { name: "flags" } });
  check(res, { "flags 200": (r) => r.status === 200 });
  return res;
}

export function liveEvents(token) {
  const res = http.get(`${BASE}/v1/live-events`, { headers: headers(token), tags: { name: "live-events" } });
  check(res, { "live-events 200": (r) => r.status === 200 });
}

export function experiments(token) {
  const res = http.get(`${BASE}/v1/experiments`, { headers: headers(token), tags: { name: "experiments" } });
  check(res, { "experiments 200": (r) => r.status === 200 });
}

// RFC 4122 v4 without webcrypto (portable across k6 versions).
export function uuid() {
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    return (c === "x" ? r : (r & 0x3) | 0x8).toString(16);
  });
}

export function sendEvents(token, n = 3) {
  const events = [];
  for (let i = 0; i < n; i++) {
    events.push({
      name: i === 0 ? "level_complete" : "shop_view",
      value: i === 0 ? "1" : undefined,
      metadata: { level: Math.floor(Math.random() * 60), source: "k6" },
      timestamp: new Date().toISOString(),
      id: uuid(),
    });
  }
  const res = http.post(`${BASE}/v1/events`, JSON.stringify({ events }), { headers: headers(token), tags: { name: "events" } });
  check(res, { "events 2xx": (r) => r.status >= 200 && r.status < 300 });
}
