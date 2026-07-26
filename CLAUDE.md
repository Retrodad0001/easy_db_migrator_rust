# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# easy_db_migrator_rust

A lightweight, plain-SQL database migration library for PostgreSQL and SQL Server.

## Scope

- see D:\src\EasyDbMigrator for the feature set

## Commands

The full check suite — fmt, clippy, docs, tests, audit, deny, semgrep, actionlint, outdated, machete — is specified once under **Validation** below and in the `jarvis` skill; run it from there rather than from a second list here. On Windows/Git Bash, prefix the Docker-based checks (semgrep, actionlint) with `MSYS_NO_PATHCONV=1` so container paths aren't mangled.

Day-to-day, beyond `cargo build`, what's worth knowing is how to narrow a test run. Each file in `tests/` compiles to its own binary named after the file:

```bash
cargo test --test postgres_integration_tests                              # one backend's whole suite
cargo test --test mssql_integration_tests can_cancel_the_migration_process  # one test
cargo test -- --nocapture                                                 # see tracing output
```

Select a backend with `--test <file>`, not a name filter — test functions are named for the scenario, not the backend, so the same scenario name exists in both files. Parallel execution is deliberate — see the Architecture rules before changing how the suite is invoked.

## Architecture

A single library crate (no binary, no workspace). `src/lib.rs` is the whole public surface — `DbMigrator`, `MigrationConfiguration`, `DatabaseKind`, `Script`, `Error`, `ClockMock`/`SystemClock`, plus a re-exported `CancellationToken` — and everything else is `pub(crate)`. `CRATE_VERSION` is `env!("CARGO_PKG_VERSION")`; it is what lands in the tracking table's `version` column, so bumping the package version changes migration rows written afterwards.

**The one flow that matters** is `DbMigrator::try_apply_migrations(kind, config, token)` in `src/migrator.rs`. It returns `bool`, never `Result` — every failure is logged via `tracing` and collapsed into `false`. In order: cancelled-token check (warn, return `true`), empty-connection-string check (return `false`), create database if missing, ensure tracking table, load+order scripts from disk, then `run_migration_scripts`. Each of those steps `match`es on `DatabaseKind` and calls into `backend::postgres` or `backend::mssql`. The private `RunOutcome` enum is what keeps "cancelled" distinguishable from "completed with failures" on the way back out — both would otherwise look the same at the `bool` boundary.

Inside `run_migration_scripts`, the first script that errors sets a `skip_due_to_previous_error` flag: the loop keeps iterating but only logs each remaining script as skipped, and the run reports failure. A cancellation detected mid-loop returns immediately with success. Those two paths are why the failure fixtures come in threes — good script, broken script, script that must be observably skipped.

`src/backend/mod.rs` holds only the shared `RunMigrationResult` enum (`MigrationScriptExecuted` / `ScriptSkippedBecauseAlreadyRun` / `MigrationWasCancelled`) and declares the two backend modules. Each backend exposes the same four `pub(crate)` async fns — `try_delete_database_if_exists`, `try_create_database_if_missing`, `try_ensure_tracking_table`, `run_script` — with no trait tying them together (see Architecture rules for why). `run_script` is where idempotence lives: it checks the tracking table for the filename first and returns `ScriptSkippedBecauseAlreadyRun` before opening a transaction, which is what makes a rerun a no-op that preserves the original timestamp.

The two backends differ in ways worth knowing before touching either:

| | `backend::postgres` (sqlx) | `backend::mssql` (tiberius) |
|---|---|---|
| Connection string | URL; database appended as `/<name>` | ADO string via `Config::from_ado_string` |
| Admin connection | connects to the `postgres` database | connects with no database selected |
| Tracking columns | `id, executed_at, filename, version` | `Id, ExecutedAt, Filename, Version` |
| Transaction | sqlx `connection.begin()` / `commit()` | explicit `BEGIN`/`COMMIT TRANSACTION`, with a best-effort `IF @@TRANCOUNT > 0 ROLLBACK` on error |

The column-casing split is a real trap: verification queries in the tests must use each dialect's casing, and a query copied from one backend's test into the other's will fail at runtime, not at compile time.

`src/script.rs` owns both parsing and ordering. `Script::parse` validates the `yyyyMMdd_NNN_name.sql` prefix byte-by-byte (rejecting empty filename, empty content, non-ASCII prefix, missing `_` separator, unparseable or non-calendar date, unparseable sequence) and `load_ordered_scripts` filters the exclusion list *before* parsing, then sorts by `date_part` then `sequence_part`. A single bad filename in the directory fails the whole load — scripts are not skipped individually.

`src/clock.rs` exists only so tests can pin `executed_at` to a fixed instant; `SystemClock` is the real implementation. `DbMigrator::with_clock_mock` is the only constructor, so the clock and exclusion list are always supplied explicitly.

## Architecture rules

Derived from `D:\src\EasyDbMigrator`'s integration tests (`PostgresServerIntegrationTests.cs`, `SqlServerIntegrationTests.cs`) — this is the observed, black-box behavioral contract the Rust port should match.

- **Forward-only migrations, no rollback.** Re-running the migrator against an already-migrated database is a true no-op: already-applied scripts are skipped, and their tracking row keeps the *original* run's timestamp — it's not upserted on rerun. There is no down/rollback path anywhere in the tested contract. Don't add up/down script pairs.
- **Script naming carries order.** Scripts are ordered by `date, then sequence` parsed from the filename (`yyyyMMdd_NNN_name.sql`), not by directory listing order or an arbitrary integer version. The filename parser must sort the same way.
- **Config never bundles the database into the connection string.** The connection string has no database selected; the database name is a separate field, because the migrator has to be able to create/drop the database itself before connecting to it.
- **Match over traits — no `DatabaseConnector` base trait.** Each dialect's operations (delete-database-if-exists, create-database-if-missing, ensure-tracking-table, run-one-script) are plain `pub(crate)` async functions in `backend::postgres` / `backend::mssql` — not a shared trait with a per-dialect `impl`. `DbMigrator` does **not** store a `DatabaseKind` as struct state; the caller passes it explicitly as a parameter to each method that needs to dispatch on it (`try_delete_database_if_exists(kind, config)`, `try_apply_migrations(kind, config, token)`), which then dispatches with `match kind { DatabaseKind::Postgresql => backend::postgres::..., DatabaseKind::Mssql => backend::mssql::... }`, instead of a `Box<dyn Trait>`. There are exactly two backends and that isn't expected to change, so a closed `match` beats a trait built for extensibility nobody needs — neither module imports the other.
- **Custom traits exist only to let tests substitute a fake — never as a general dispatch mechanism.** If there's a fixed, known set of real implementations (see the rule above), use an enum + `match`, not a trait. The one legitimate reason to define a trait in this crate is so a test can inject deterministic/fake behavior in place of something normally real (e.g. wall-clock time) — and when that's the reason, the trait name ends in `Mock` (PascalCase, e.g. `ClockMock`) so it's unmistakable which traits exist purely to enable test doubles. (A literal snake_case `_mock` suffix would trip the `non_camel_case_types` lint, which `cargo clippy --all-targets -- -D warnings` denies — hence `Mock`, not `_mock`.)
- **Exclusion list.** Support excluding specific migrations by filename from a run, filtered out before ordering/execution — not skipped silently mid-run.
- **Cancellation is not failure.** A run cancelled before/during execution should log a warning and return success, not an error — the .NET contract returns `true` even when the token was already cancelled.
- **Tracking table columns:** `id, executed_at, filename, version` — `version` is this crate's own version stamped per row, not a per-migration version. Table name `DbMigrationsRun` for parity with the .NET original unless there's a reason to diverge.
- **Both backends always compile in — no Cargo features, no `#[cfg(feature = ...)]`, ever.** `backend::postgres` and `backend::mssql` are plain, always-present modules (single crate, no workspace split — matches the .NET project's clean split between `PostgreSqlConnector`/`MicrosoftSqlConnector` without the ceremony of separate crates). `sqlx` and `tiberius` are mandatory `[dependencies]`, not optional/feature-gated. `#[cfg(feature = "postgres")]`, `#[cfg(feature = "mssql")]`, or any other `cfg(feature = ...)` must never appear anywhere in this crate — not to gate compilation, not to branch logic, not for anything. Runtime database selection goes entirely through `pub enum DatabaseKind { Mssql, Postgresql }`, passed in explicitly by the caller to each `DbMigrator` method that dispatches on it, not stored on the migrator itself.
- **Integration tests use real containers**, one per test (via `testcontainers`), mirroring the .NET project's `TestEnvironment.Docker` usage — no mocked DB connections in integration tests.
- **Every code path gets an explicit test.** Each branch a function can take (success, each distinct error/failure case, each early-return, each match arm) needs a test that actually drives execution down that specific branch — not just a test that happens to compile-check it or exercise a different branch that incidentally covers the same lines. E.g. a script-execution-failure path and a mid-run-cancellation path are each their own test, not assumed covered by a pre-run-cancellation test or a happy-path test.
- **Cross-platform: this crate must build and run on macOS, Windows, and Linux.** Don't add code, dependencies, or CI steps that only work on one OS without an equivalent path for the others. Note the practical tension this creates with `tiberius`'s `native-tls` feature (see Code rules): it delegates to the OS's own TLS stack (Schannel/Windows, Secure Transport/macOS, OpenSSL/Linux), so Linux build and CI environments need a working OpenSSL installation available — unlike the `rustls` feature it replaced, which was pure-Rust with no OS-level TLS dependency. `ci.yml`/`ci_weekly.yml` currently only run on `ubuntu-latest`; if that stops being sufficient to catch a platform-specific regression, add `windows-latest`/`macos-latest` to the build matrix rather than assuming Linux CI covers all three platforms.
- **Integration tests run in parallel, each spinning up its own container.** No `#[serial]`/`serial_test`, no shared/global container, and no `--test-threads=1` — every test starts its own `MssqlServer`/`Postgres` container instance and uses a randomized database name (see `test_random_database_name`) so concurrent runs can't collide.
- **PostgreSQL and SQL Server test fixtures are kept in separate per-backend directory trees, never mixed.** Each backend's `.sql` fixture scripts live under its own `tests/fixtures/<backend>/` root (`tests/fixtures/postgres/test_scripts` + `tests/fixtures/postgres/test_scripts_failure`, `tests/fixtures/mssql/test_scripts` + `tests/fixtures/mssql/test_scripts_failure`) — a postgres test never points at an mssql fixture dir or vice versa, and scripts written in one dialect's SQL never share a directory with the other's. Add new fixtures under the matching backend root; don't reintroduce a flat shared folder distinguished only by filename prefix.

## Code rules

> Several general Rust code rules for this crate now live in the personal **`jarvis`** skill (see the Skills section): `pub(crate)` internals, no `.unwrap()`/`.expect()`, the no-panic/indexing/wildcards lint bundle, the no-comments rule, `cargo fmt` before done, test-only crates in `[dev-dependencies]`, the `test_` helper prefix, `#[cfg(test)]` on every test, and the no-zero-arg-constructor / fields-via-constructor / no-`Default` construction rules. They still apply; they're just maintained there. The rules below are the ones kept project-local.

- One crate-wide `Error` enum in `error.rs`, built with `thiserror`. Backend-specific variants (e.g. `Postgres(sqlx::Error)`, `Mssql(tiberius::error::Error)`) live in that same enum — don't scatter separate error types per module.
- **Async runtime: `tokio` only.** Don't pull in a second runtime (e.g. `async-std`, `smol`) anywhere, including dev-dependencies.
- **Logging: `tracing`**, not an injected logger. The .NET original threads `ILogger` through constructors explicitly; the Rust port uses `tracing`'s ambient spans/events instead — this is an intentional idiom deviation, not an oversight. Log at the same semantic points the Architecture rules call out (e.g. cancellation warning) using `tracing::warn!`/`info!`/`error!`.
- **Remove unused crates.** If a change stops referencing a dependency anywhere in `src/` or `tests/`, delete it from `Cargo.toml` in the same change — don't leave stale entries behind, whether from a `cargo add` that turned out unnecessary or code that got refactored away.
- **No `.await?` (or bare `?`) in test functions.** Test fns must not return `Result` and let a setup/assertion failure silently propagate out via `?` — that only surfaces the harness's generic Debug-formatted error. Fail with an explicit, descriptive assertion instead: `let Ok(value) = fallible().await else { panic!("what failed and why: {reason}") };`, or `assert!`/`assert_eq!` with a format-string message. This is test-only — it doesn't relax the crate-wide `.unwrap()`/`.expect()` ban (kept in the `jarvis` skill), which stays denied everywhere, including tests.
  - `panic!()` in `tests/*.rs` is a confirmed, deliberate exception to the crate-wide `clippy::panic = "deny"`, not an oversight. It was investigated: `assert!(false, "msg")` doesn't type-check as a substitute in a function returning a generic `T` (e.g. `test_expect_ok<T, E>(...) -> T`) — `assert!` always has type `()`, never the diverging `!` type the compiler needs there. Even where `assert!(false, ...)` does compile (inside a `()`-returning function, followed by `return;`), `clippy::assertions_on_constants` fires and its own suggested fix is `panic!()`/`unreachable!()` — both already banned. The only real panic-free escape is reversing this rule so test functions return `Result<(), E>` and propagate with `?`, which was considered and rejected to keep descriptive failure messages. So `#![allow(clippy::panic, missing_docs)]` stays file-wide in `tests/*.rs`; don't try to "fix" this again without re-confirming with the user first.

## Validation

> Part of the validation suite now lives in the personal **`jarvis`** skill (see the Skills section): `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, "don't report done on a compile alone", `cargo audit`, `cargo deny check`, `actionlint`, Semgrep, and `cargo outdated`. A full validation run must cover both those and the project-local checks below.

- **Docs:** `cargo doc --no-deps` must build without warnings, since public items are required to have doc comments.
- **Unit tests:** `cargo test --lib` — no Docker required, must always run and pass.
- **Integration tests:** require a running Docker daemon (see Architecture rules — real containers via `testcontainers`). Run `cargo test` — both backends' integration tests run in the same invocation now that both are always compiled in. If Docker isn't available in the current environment, say so explicitly rather than reporting tests as passing — never assume/claim integration coverage without actually running it.
- **Untested code must be flagged.** Any new or changed code path that no test actually exercises (e.g. integration tests that only compile-checked because Docker wasn't available, an error branch nothing triggers, a fixture that was never run against) must be called out explicitly to the user as untested — don't let compiling or "looks correct" stand in for having run it.
- **Toolchain/tool freshness:** `rustup check` must report the Rust toolchain up to date. Semgrep has no version to go stale since it's always run via `semgrep/semgrep:latest` (`docker pull semgrep/semgrep:latest` first if there's any doubt the local image is current) rather than a pinned tag.
- **Unused dependencies:** `cargo machete` (installed via `cargo install cargo-machete --locked` if not already present) must report no unused dependencies. This is a stopgap: clippy and rustc have no reliable equivalent today (`unused_crate_dependencies` is a rustc lint, not clippy, and is per-compilation-target so it false-positives on deps only used by one target, e.g. a dev-dependency used only in `tests/`). Drop this step in favor of a built-in toolchain check if/when one lands that doesn't have that false-positive problem.

## Agent rules

> All agent rules now live in the personal **`jarvis`** skill (see the Skills section): no deleting `//TODO`/`//FIXME`/`//BUG`; the user decides which new tests are added; never change/delete an existing test (or its fixtures/helpers) without asking; ask expected behaviour before writing a test; never change `Cargo.toml` without asking; and commit/push straight to `main` with no feature branches. They still apply; they're just maintained there.

## Skills

- **`jarvis`** — a personal (user-scoped) skill, defined in `~/.claude/skills/jarvis/SKILL.md` (that path is a junction to `D:\_DEVTOOLS\Skills\jarvis` on the user's machine, so the skill files actually live under `D:\_DEVTOOLS\Skills`). Being personal-scoped it's available in every project, not just this one; invoke it with `/jarvis` or let it auto-trigger from its `description`. It holds the Rust code-quality rules, part of the validation suite, and all the agent rules moved out of this `CLAUDE.md` (see the `>` pointers in those sections). NOTE: because the skill lives outside the repo, those rules are machine-local and not version-controlled with the project; and a Claude Code restart is needed before a newly added/edited skill is picked up.
