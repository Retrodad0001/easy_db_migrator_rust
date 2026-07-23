# easy_db_migrator_rust

A lightweight, plain-SQL database migration library for PostgreSQL and SQL Server.

## Scope

- see D:\src\EasyDbMigrator for the feature set

## Architecture rules

Derived from `D:\src\EasyDbMigrator`'s integration tests (`PostgresServerIntegrationTests.cs`, `SqlServerIntegrationTests.cs`) — this is the observed, black-box behavioral contract the Rust port should match.

- **Forward-only migrations, no rollback.** Re-running the migrator against an already-migrated database is a true no-op: already-applied scripts are skipped, and their tracking row keeps the *original* run's timestamp — it's not upserted on rerun. There is no down/rollback path anywhere in the tested contract. Don't add up/down script pairs.
- **Script naming carries order.** Scripts are ordered by `date, then sequence` parsed from the filename (`yyyyMMdd_NNN_name.sql`), not by directory listing order or an arbitrary integer version. The filename parser must sort the same way.
- **Config never bundles the database into the connection string.** The connection string has no database selected; the database name is a separate field, because the migrator has to be able to create/drop the database itself before connecting to it.
- **Match over traits — no `DatabaseConnector` base trait.** Each dialect's operations (delete-database-if-exists, create-database-if-missing, ensure-tracking-table, run-one-script) are plain `pub(crate)` async functions in `backend::postgres` / `backend::mssql` — not a shared trait with a per-dialect `impl`. `DbMigrator` holds a `DatabaseKind` and dispatches with `match self.kind { DatabaseKind::Postgresql => backend::postgres::..., DatabaseKind::Mssql => backend::mssql::... }` at each call site, instead of a `Box<dyn Trait>`. There are exactly two backends and that isn't expected to change, so a closed `match` beats a trait built for extensibility nobody needs — neither module imports the other.
- **Custom traits exist only to let tests substitute a fake — never as a general dispatch mechanism.** If there's a fixed, known set of real implementations (see the rule above), use an enum + `match`, not a trait. The one legitimate reason to define a trait in this crate is so a test can inject deterministic/fake behavior in place of something normally real (e.g. wall-clock time) — and when that's the reason, the trait name ends in `Mock` (PascalCase, e.g. `ClockMock`) so it's unmistakable which traits exist purely to enable test doubles. (A literal snake_case `_mock` suffix would trip the `non_camel_case_types` lint, which `cargo clippy --all-targets -- -D warnings` denies — hence `Mock`, not `_mock`.)
- **Exclusion list.** Support excluding specific migrations by filename from a run, filtered out before ordering/execution — not skipped silently mid-run.
- **Cancellation is not failure.** A run cancelled before/during execution should log a warning and return success, not an error — the .NET contract returns `true` even when the token was already cancelled.
- **Tracking table columns:** `id, executed_at, filename, version` — `version` is this crate's own version stamped per row, not a per-migration version. Table name `DbMigrationsRun` for parity with the .NET original unless there's a reason to diverge.
- **Both backends always compile in — no Cargo features, no `#[cfg(feature = ...)]`, ever.** `backend::postgres` and `backend::mssql` are plain, always-present modules (single crate, no workspace split — matches the .NET project's clean split between `PostgreSqlConnector`/`MicrosoftSqlConnector` without the ceremony of separate crates). `sqlx` and `tiberius` are mandatory `[dependencies]`, not optional/feature-gated. `#[cfg(feature = "postgres")]`, `#[cfg(feature = "mssql")]`, or any other `cfg(feature = ...)` must never appear anywhere in this crate — not to gate compilation, not to branch logic, not for anything. Runtime database selection goes entirely through `pub enum DatabaseKind { Mssql, Postgresql }`, passed in explicitly by the caller to `DbMigrator::new`/`with_clock`.
- **Integration tests use real containers**, one per test (via `testcontainers`), mirroring the .NET project's `TestEnvironment.Docker` usage — no mocked DB connections in integration tests.

## Code rules
- All internal methods and functions should be `pub(crate)` when not used outside this crate; only the actual public API surface is `pub`.
- No `.unwrap()` / `.expect()` — enforced via `[lints.clippy] unwrap_used/expect_used = "deny"` in `Cargo.toml`. Propagate errors with `?` instead.
- One crate-wide `Error` enum in `error.rs`, built with `thiserror`. Backend-specific variants (e.g. `Postgres(sqlx::Error)`, `Mssql(tiberius::error::Error)`) live in that same enum — don't scatter separate error types per module.
- **Async runtime: `tokio` only.** Don't pull in a second runtime (e.g. `async-std`, `smol`) anywhere, including dev-dependencies.
- **Logging: `tracing`**, not an injected logger. The .NET original threads `ILogger` through constructors explicitly; the Rust port uses `tracing`'s ambient spans/events instead — this is an intentional idiom deviation, not an oversight. Log at the same semantic points the Architecture rules call out (e.g. cancellation warning) using `tracing::warn!`/`info!`/`error!`.
- Public API items (anything `pub`) get a `///` doc comment stating what it does; skip doc comments on `pub(crate)`/private items unless the WHY is non-obvious.
- Run `cargo fmt` (not just `--check`) before considering any code change done, so formatting is actually applied and not just verified.
- **Test-only crates go in `[dev-dependencies]`.** Anything pulled in solely for tests (e.g. `testcontainers`, `testcontainers-modules`, `tracing-test`, `rand`) belongs under `[dev-dependencies]`, never `[dependencies]` — it must not ship in the built library. `sqlx`/`tiberius` are the exception: they're real `[dependencies]` (mandatory, not feature-gated) because the backends need them at runtime, not just in tests.
- **Remove unused crates.** If a change stops referencing a dependency anywhere in `src/` or `tests/`, delete it from `Cargo.toml` in the same change — don't leave stale entries behind, whether from a `cargo add` that turned out unnecessary or code that got refactored away.
- **No `.await?` (or bare `?`) in test functions.** Test fns must not return `Result` and let a setup/assertion failure silently propagate out via `?` — that only surfaces the harness's generic Debug-formatted error. Fail with an explicit, descriptive assertion instead: `let Ok(value) = fallible().await else { panic!("what failed and why: {reason}") };`, or `assert!`/`assert_eq!` with a format-string message. This is test-only — it doesn't relax rule 25 (`.unwrap()`/`.expect()` stay denied everywhere, including tests).
- **Test helper functions are prefixed `test_`.** Any function in a `tests/*.rs` file that isn't itself a `#[tokio::test]`/`#[test]`-annotated test (setup helpers, fixtures, assertion helpers — e.g. `fixtures_dir`, `random_database_name`, `datetime`, `expect_ok`, `fetch_tracking_rows`) is named `test_<thing>` so it's unmistakable at the call site that it's test scaffolding, not library code. The `#[tokio::test]` functions themselves keep their descriptive scenario names (e.g. `can_cancel_the_migration_process`) — the prefix is only for the helpers they call.
- **Every test function gets `#[cfg(test)]`, even when `#[tokio::test]` is already present.** Stack it directly on the function (`#[cfg(test)] #[tokio::test] async fn ...`) rather than relying solely on an enclosing `#[cfg(test)] mod tests { ... }` to gate it out of non-test builds.
- **Test function names are prefixed with the database kind.** e.g. `mssql_when_nothing_goes_wrong_with_running_the_migrations_on_an_empty_database`, `postgres_can_skip_scripts_if_they_already_ran_before` — so which backend a test targets is unmistakable from the name alone, without opening the file.

## Validation

- **Format:** `cargo fmt --check` must be clean.
- **Lint:** `cargo clippy --all-targets -- -D warnings` must be clean.
- **Docs:** `cargo doc --no-deps` must build without warnings, since public items are required to have doc comments.
- **Unit tests:** `cargo test --lib` — no Docker required, must always run and pass.
- **Integration tests:** require a running Docker daemon (see Architecture rules — real containers via `testcontainers`). Run `cargo test` — both backends' integration tests run in the same invocation now that both are always compiled in. If Docker isn't available in the current environment, say so explicitly rather than reporting tests as passing — never assume/claim integration coverage without actually running it.
- Don't report a task done based on a build/compile check alone — actually run the relevant test command(s) above and confirm they pass.
- **Untested code must be flagged.** Any new or changed code path that no test actually exercises (e.g. integration tests that only compile-checked because Docker wasn't available, an error branch nothing triggers, a fixture that was never run against) must be called out explicitly to the user as untested — don't let compiling or "looks correct" stand in for having run it.
