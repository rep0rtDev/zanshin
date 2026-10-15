# Migrating from Satori

Zanshin covers everything Satori offers (flags, audiences, experiments, live events, messaging,
identities, analytics, data lakes) and more. Satori itself is closed source and has no public
export format, so migration is **best effort**: `zanshinctl import satori` understands the JSON
shapes returned by Satori's client API and the console exports we have seen, maps them to a
Zanshin config bundle and shows a plan before writing anything.

```sh
zanshinctl import satori ./satori-export.json --dry-run   # mapping report, nothing written
zanshinctl import satori ./satori-export.json             # apply
```

## What maps where

| Satori | Zanshin | Notes |
| --- | --- | --- |
| Feature flag (`name`, `value`, conditions) | Flag with `default_value`; conditions → rules | value type inferred from the JSON value; Satori stores values as strings, use `--infer-types` to turn `"true"` / `"10"` into bool / number |
| Audience (name, filter expression) | Audience with the expression editor | Satori's expression dialect is translated for `Properties*`, `SemverMatch`, `Now`, `Duration`; untranslatable filters are kept as comments in the report |
| Experiment (name, value, variants) | Experiment with variants and a `value` metric | assignments are not migrated (players are re-bucketed deterministically) |
| Live event (`active_start_time_sec`, `duration_sec`, `reset_cron`, `value`) | Live event with schedule | one-off and recurring schedules supported |
| Message template / schedule | Message campaign | push credentials are configured separately under *Integrations* |
| Identity properties (`default`, `computed`, `custom`) | Same three property classes | identities are imported through the server API (`POST /server/v1/identities`), not the config bundle |
| Events | Same event model (`name`, `value`, `metadata`, `timestamp`) | historical events: export from Satori's data lake and ingest with `zanshinctl events import` |

## SDK changes

Client calls map one-to-one (`authenticate`, `getFlags`, `getLiveEvents`, `event`, `identify`,
`updateProperties`, `getExperiments`, `getMessages`). The Nakama runtime module replaces Satori's
Nakama integration without code changes on the client.

If your export does not import cleanly, open an issue with a redacted sample — the mapper is
extended for every shape we see.
