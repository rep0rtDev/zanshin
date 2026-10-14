# Zanshin admin console

React + Vite single-page app served by the Zanshin server at `/console`. It talks to the
`/admin/v1` API with an admin API key that is kept in the browser's local storage only.

Screens: overview, feature flags (rules editor, per-identity overrides, dry run), experiments
(lifecycle, results with any event as conversion metric), audiences (membership check),
live events, message campaigns (delivery stats), identities (properties, events, inbox with
direct send, push devices), analytics (event names + time series), API keys.

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
