# Zanshin admin console

React + Vite single-page app served by the Zanshin server at `/console`. It talks to the
`/admin/v1` API with an admin API key that is kept in the browser's local storage only.

Screens: overview, feature flags (visual rule builder with typed values / weighted variants,
per-identity overrides, dry run), experiments (lifecycle, results with any event as conversion
metric and lift vs control), audiences (condition builder, membership check), live events
(month calendar of server-computed occurrences), message campaigns (live template preview,
delivery stats), identities (properties, events, inbox with direct send, push devices),
analytics (compare up to four events, events vs unique identities, hover tooltips), API keys
(role / expiring / per-key quota), audit log (filters, cursor paging, redacted bodies). Sign in with an
admin API key or, when `admin.oidc` is configured, through the SSO button; the sidebar shows the
current user and role, and the API keys screen is only offered to admins.
Light and dark themes (follows the OS by default).

The targeting condition editor (`src/RuleBuilder.tsx`) edits the same JSON the API accepts
(`all` / `any` / `not` groups and `prop` / `op` / `value` comparisons) and can be toggled to raw
JSON at any time.

## Development

```sh
cd console
npm ci
npm run dev        # http://localhost:5173/console/ — proxies /admin to a server on :7550
npm run lint       # tsc -b
npm run build      # writes dist/, which `cargo build` embeds into the zanshin binary
```

The server embeds `console/dist` at compile time (`rust-embed`). Build the console before
`cargo build` to get a binary with the UI; without it `/console` answers 503. The Dockerfile
and CI do both steps.

## End-to-end tests

`console/e2e` holds Playwright tests that drive the built console against a real server (the CI job
`console-e2e` builds the console, embeds it into the server, starts it on `:7560` and runs them).
Locally:

```sh
npm run build && (cd .. && cargo build -p zanshin-server --bin zanshin)
ZANSHIN__ADMIN__BOOTSTRAP_KEY=e2e-bootstrap-admin-key-0001 ZANSHIN__SERVER__HTTP_ADDR=127.0.0.1:7560 ../target/debug/zanshin &
npx playwright install chromium   # once; or PW_EXECUTABLE=/path/to/chrome to use an installed browser
npm run e2e                       # ZANSHIN_URL / ADMIN_KEY override the defaults
```
