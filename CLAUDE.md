# easy_db_migrator_rust

A lightweight, plain-SQL database migration library for PostgreSQL and SQL Server.

## Scope

- see D:\src\EasyDbMigrator for the feature set

## Architecture rules

Derived from `D:\src\EasyDbMigrator`'s integration tests (`PostgresServerIntegrationTests.cs`, `SqlServerIntegrationTests.cs`) — this is the observed, black-box behavioral contract the Rust port should match.

- **Forward-only migrations, no rollback.** Re-running the migrator against an already-migrated database is a true no-op: already-applied scripts are skipped, and their tracking row keeps the *original* run's timestamp — it's not upserted on rerun. There is no down/rollback path anywhere in the tested contract. Don't add up/down script pairs.
- **Script naming carries order.** Scripts are ordered by `date, then sequence` parsed from the filename (`yyyyMMdd_NNN_name.sql`), not by directory listing order or an arbitrary integer version. The filename parser must sort the same way.
- **Config never bundles the database into the connection string.** The connection string has no database selected; the database name is a separate field, because the migrator has to be able to create/drop the database itself before connecting to it.
- **One connector trait, one impl per dialect.** A backend trait covers: delete-database-if-exists, create-database-if-missing, ensure-tracking-table, run-one-script. Postgres and SQL Server each get their own impl; neither impl imports the other (see feature-gating below).
- **Exclusion list.** Support excluding specific migrations by filename from a run, filtered out before ordering/execution — not skipped silently mid-run.
- **Cancellation is not failure.** A run cancelled before/during execution should log a warning and return success, not an error — the .NET contract returns `true` even when the token was already cancelled.
- **Tracking table columns:** `id, executed_at, filename, version` — `version` is this crate's own version stamped per row, not a per-migration version. Table name `DbMigrationsRun` for parity with the .NET original unless there's a reason to diverge.
- **Backend isolation via Cargo features.** `backend::postgres` behind `feature = "postgres"`, `backend::mssql` behind `feature = "mssql"` — neither may import the other. No workspace split (single crate, feature-gated modules) — matches the .NET project's clean split between `PostgreSqlConnector`/`MicrosoftSqlConnector` without the ceremony of separate crates.
- **Integration tests use real containers**, one per test (via `testcontainers`), mirroring the .NET project's `TestEnvironment.Docker` usage — no mocked DB connections in integration tests.

## Code rules
- All internal methods and functions should be `pub(crate)` when not used outside this project; only the actual public API surface is `pub`.
- No `.unwrap()` / `.expect()` — enforced via `[lints.clippy] unwrap_used/expect_used = "deny"` in `Cargo.toml`. Propagate errors with `?` instead.
- One crate-wide `Error` enum in `error.rs`, built with `thiserror`. Backend-specific variants (e.g. `Postgres(sqlx::Error)`, `Mssql(tiberius::error::Error)`) are `#[cfg(feature = ...)]`-gated, same as the modules that produce them — don't scatter separate error types per module.
- **Async runtime: `tokio` only.** Don't pull in a second runtime (e.g. `async-std`, `smol`) anywhere, including dev-dependencies.
- **Logging: `tracing`**, not an injected logger. The .NET original threads `ILogger` through constructors explicitly; the Rust port uses `tracing`'s ambient spans/events instead — this is an intentional idiom deviation, not an oversight. Log at the same semantic points the Architecture rules call out (e.g. cancellation warning) using `tracing::warn!`/`info!`/`error!`.
- Public API items (anything `pub`) get a `///` doc comment stating what it does; skip doc comments on `pub(crate)`/private items unless the WHY is non-obvious.

## Validation

- **Format:** `cargo fmt --check` must be clean.
- **Lint:** `cargo clippy --all-features --all-targets -- -D warnings` must be clean. Backends are feature-gated, so plain `cargo clippy` alone doesn't touch `backend::postgres`/`backend::mssql` — always include `--all-features` (or explicitly check each of `--no-default-features`, `--features postgres`, `--features mssql` when isolating a feature-specific change).
- **Docs:** `cargo doc --all-features --no-deps` must build without warnings, since public items are required to have doc comments.
- **Unit tests:** `cargo test --all-features` — no Docker required, must always run and pass.
- **Integration tests:** require a running Docker daemon (see Architecture rules — real containers via `testcontainers`). Run per-backend: `cargo test --features postgres` / `--features mssql`. If Docker isn't available in the current environment, say so explicitly rather than reporting tests as passing — never assume/claim integration coverage without actually running it.
- Don't report a task done based on a build/compile check alone — actually run the relevant test command(s) above and confirm they pass.
