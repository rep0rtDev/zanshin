# Contributing

Zanshin is Apache-2.0 and welcomes contributions of every size.

## Development setup

```sh
docker compose up -d postgres redis clickhouse
export ZANSHIN__AUTH__JWT_SECRET=dev-secret-dev-secret-dev-secret-0123456789
export ZANSHIN__ADMIN__BOOTSTRAP_KEY=dev-admin-key-00000000
cargo run -p zanshin-server
```

`cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`
must pass before a PR is opened. Integration tests run only when `ZANSHIN_TEST_DATABASE_URL` and
`ZANSHIN_TEST_REDIS_URL` are set (CI sets them).

## Layout

| Path | Purpose |
|------|---------|
| `crates/zanshin-core` | Domain types and IO-free logic (tokens, property rules, validation) |
| `crates/zanshin-store` | PostgreSQL / Redis / ClickHouse access |
| `crates/zanshin-proto` | Protobuf definitions and generated gRPC stubs |
| `crates/zanshin-server` | The `zanshin` binary: config, REST (axum), gRPC (tonic), services |
| `proto/` | `.proto` sources shared with every SDK |
| `migrations/` | SQL migrations, embedded into the binary and applied on start |

## Conventions

* Business logic lives in `zanshin-server/src/services`; REST and gRPC handlers only translate.
* Errors are `zanshin_core::Error` everywhere and are mapped to HTTP / gRPC status codes once.
* New tables get a new migration file; never edit an applied migration.
* Public API changes update `proto/` and `docs/api.md` in the same PR.
