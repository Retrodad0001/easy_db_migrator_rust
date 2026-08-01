# Architecture

## Context

this is a database migration tool for Microsoft SQL server and PostgreSQL and
can be used by other rust projects

It doubles as a framework for integration testing: a project points it at its own
migration scripts to build the database a test needs, runs the test against it,
and drops it again afterwards. Both uses are first-class, so the API stays usable
from test code and not only from an application's startup path.

## Deployment

There is nothing to deploy. This is a library crate — `src/lib.rs`, no binary
target — so it ships as a dependency inside whatever application consumes it,
and that application's deployment is the only one in play. It has no database,
UI, or running service of its own.

`Cargo.toml` sets `publish = false`, so it is not on crates.io today: a consumer
takes it by git URL or local path rather than by a version from the registry.

## Tooling

Every framework, library, and tool this project commits to, and why it was chosen.
The user decides what goes here; nothing enters `Cargo.toml` before it is listed.

- **PostgreSQL** — one of the two databases this tool migrates. It is a target,
  not this project's own datastore; the only thing the tool keeps there is its
  `DbMigrationsRun` tracking table.
- **Microsoft SQL Server** — the other target database. Both backends are always
  compiled in, so neither is hidden behind a Cargo feature.
- **sqlx** — the PostgreSQL client. Migration scripts are plain SQL run through
  the runtime query API; no ORM, and deliberately not the compile-time-checked
  macros, because the database a migration targets does not exist at build time.
- **tiberius** — the SQL Server client, chosen on the same terms: plain SQL, no
  ORM.
- **tokio** — the async runtime everything above runs on. It is the only runtime
  the crate will take on; a second one is never added, tests included.
- **tokio-util** — two things: `CancellationToken`, which the public API
  re-exports so a caller can stop a run part-way, and the `compat` adapter that
  lets tiberius accept a tokio `TcpStream`.
- **chrono** — date and time types. Parses the date in a script filename and
  stamps `executed_at` on each tracking row.
- **thiserror** — derives the single crate-wide `Error` enum in `error.rs`.
- **tracing** — how the library reports what it did, rather than an injected
  logger. Failures are logged and then collapsed into a `bool`, so the log is the
  only account a caller gets of what actually happened.
- **testcontainers** and **testcontainers-modules** — start the real PostgreSQL
  and SQL Server containers the integration tests run against, one container per
  test so the suite can run in parallel.
- **rand** — generates the random database name each integration test targets,
  which is what keeps those parallel tests from colliding.
- **tracing-test** — captures `tracing` output inside a test, so the log records
  a run emitted can be asserted like any other observable effect.
- **Docker Desktop** — runs the containers the local integration tests start, and
  the containerised checks in the Quality checks section.

## Reference code and documentation

Documentation and example code worth consulting for the tooling above — the
sources to check before guessing at an API.

- **tokio** — <https://tokio.rs/> and <https://docs.rs/tokio/latest/tokio/>
- **sqlx** — <https://docs.rs/sqlx/latest/sqlx/>
- <https://crates.io/crates/tiberius>
- <https://www.docker.com/>

## Code rules

The rules the code in this repo follows. Lint denials are declared in
`Cargo.toml` under `[lints.rust]` and `[lints.clippy]` and enforced by `cargo
clippy`; the rules below are the ones no lint can check.

- **Nothing beyond what Context describes** — the crate stays a library that
  migrates Microsoft SQL Server and PostgreSQL, usable from another Rust project.
  It grows no UI, no HTTP or web layer, no long-running service, no datastore of
  its own, and no third database backend. Widening what this crate is starts by
  agreeing the change and rewriting Context; the code follows that, never the
  other way round.
- **Visibility is `pub(crate)` by default** — every function, method, and type is
  `pub(crate)` unless it is part of the crate's actual public API surface; only
  those are `pub`.
- **`#[cfg(test)]` on every test function** — stacked directly on the function
  (`#[cfg(test)] #[tokio::test] async fn ...`), even when an enclosing
  `#[cfg(test)] mod tests` already gates it out of non-test builds.
- **No zero-parameter constructors** — a `new()`, or any other associated
  constructor, taking no arguments must not exist; every constructor takes at
  least one meaningful parameter.
- **Fields are set through the constructor** — every field a struct holds is
  supplied as a constructor parameter, never populated afterwards by a
  `&mut self` setter method.
- **No blanket `Default` impls** — `impl Default` is a zero-parameter constructor
  by another name, and it hides required configuration behind an implicit choice.
  Construction stays explicit: callers pass every value at the call site.

## Quality checks

How a change is verified before it counts as done. Every gate a change must pass
is listed below, together with how it is run in this repo.

- **The code matches the Context section** — what the crate does still matches
  what Context says it is. A module, public item, or binary target that only
  makes sense for something Context does not describe is the signal to stop, and
  so is a line in Context that no code backs up any more. Both are reported to
  the user rather than quietly reconciled either way. No tool enforces this one;
  it is checked by reading.
- **The code matches the Tooling section** — nothing is used that Tooling does not
  list. Every crate in `Cargo.toml` traces back to an entry there, and a library
  that turns out to be needed is agreed and written down before it is used, not
  justified afterwards. No tool enforces this one; it is checked by reading.
- **Formatting** — `cargo fmt --check` must be clean across every `.rs` file in
  the crate, tests included. Formatting is applied by running `cargo fmt`, never
  adjusted by hand and never argued with; rustfmt's output is the house style.
- **No warnings** — every linter used here must finish with no errors *and* no
  warnings, not merely a zero exit code. A warning is fixed at its cause, never
  baselined and never hidden behind an inline suppression; where a suppression is
  genuinely the right answer, it is agreed with the user first and states why.
- **Markdown** — every `.md` file in the repo must pass `markdownlint`, run via
  Docker (`davidanson/markdownlint-cli2`) like the other containerised checks.
- **Static analysis** — `semgrep` must report zero findings, run via Docker with
  `target/` excluded. It scans only the files git tracks, so a brand-new file
  proves nothing until it is staged — check what it actually scanned, not just
  the finding count.
- **Workflow linting** — `actionlint` must exit clean, run via Docker
  (`rhysd/actionlint:latest`) like the other containerised checks. Any change
  under `.github/workflows/` is checked with it before that change counts as
  done. Nothing else validates those files: a broken workflow otherwise announces
  itself on the next push, once the mistake is already committed.
- **Unused dependencies** — `cargo machete` must find nothing. A crate listed in
  `Cargo.toml` that no code imports is removed rather than left to age into an
  advisory or a licence obligation nobody remembers taking on.
- **Documentation builds** — `cargo doc --no-deps` must succeed with no warnings,
  including no broken intra-doc links. Every public item carries a doc comment
  already; this proves those comments still resolve to the items they reference.
- **Dependency advisories** — `cargo audit` must report no warnings and no
  vulnerabilities. Unmaintained or yanked crates count as warnings and are dealt
  with, not ignored; an advisory is never silenced by an allow-list entry without
  the user agreeing to it first.
- **Licences, bans, and sources** — `cargo deny check` must pass all of its
  gates, not only the advisories one. Its configuration is `deny.toml` at the
  repo root, and that file is the single place a decision is recorded; a crate is
  never let through by relaxing a check on the command line instead. Adding an
  exception, allowing a licence, or skipping an advisory there is agreed with the
  user first, and says why in the file.
- **Dependency freshness** — `cargo outdated --root-deps-only` must report
  nothing behind a newer release. Alone among the gates here this one is not a
  per-change check: a crate goes stale because someone else published, never
  because of anything in the commit, so it runs on the weekly schedule
  (`ci_weekly.yml`) instead of on every push. It covers the direct dependencies
  only — a transitive version is not something this repo can bump on its own. A
  dependency left to drift becomes an advisory or a painful upgrade later, so the
  bump is taken while it is still small. Where an upgrade genuinely cannot be
  taken yet, that is agreed with the user and the reason written down — never
  passed over in silence.
- **Toolchain freshness** — `rustup check` must report the stable toolchain, and
  `rustup` itself, up to date. Like dependency freshness this one is cadence-based
  rather than per-change: it runs at most once a calendar week, on the first task
  of that week, and is skipped if that week's check already happened. What it
  finds is reported and the user decides what to do about it — `rustup update` is
  never run, and a pinned `rust-toolchain.toml` never edited, without approval
  for that specific action.
- **Every behaviour is proven by a test** — a feature is not done because it
  compiles and appeared to work when run by hand against a database; it is done
  when a test fails without it and passes with it. Shipping behaviour no test
  exercises is the one thing this section exists to prevent.
- **Every code path is exercised by an integration test** — the integration
  suites are what prove the code does its job against a real database, so a
  function, match arm, or error path that no integration test reaches counts as
  untested even where a unit test covers it. Code nothing in `tests/` drives is
  reported to the user as uncovered, together with a specific test proposed for
  it: which suite it belongs in, the scenario it sets up, and what it asserts.
  The test is proposed for review and never added unasked — the user decides
  which tests get written. No tool enforces this one; it is checked by reading.
- **Tests** — `cargo test` must finish with no failures and nothing skipped. An
  `#[ignore]`d test does not count as passing, and a run that executed zero tests
  is not evidence that anything works.
- **Integration container tests** — both suites,
  `tests/postgres_integration_tests.rs` and `tests/mssql_integration_tests.rs`,
  run in full against real database containers and must come back with no
  errors, no failures, and no warnings — warnings raised while compiling the test
  targets included. Docker has to be running: a container that would not start is
  a failed run, never a skipped check. Running one backend's suite says nothing
  about the other, and a suite that did not execute is never reported as passing.
- **Integration tests assert every observable effect** — a container test proves
  what the run actually did, not merely that it finished, and checks all three of
  these every time. The database: the rows in the tracking table and the tables
  the scripts were meant to create, and just as importantly the ones a skipped or
  failed script must not have created. The log: the `tracing` records the run
  emitted, including the absence of a record that should not appear. The return
  value: `Ok` where the run is expected to succeed, and where it is expected to
  fail an `Err` whose message is asserted exactly and shown to be the same text
  the run logged. Checking one of the three passes happily while the other two
  are wrong.

## Agent rules

How an agent works in this repo — including that every gate in Quality checks
must pass before a change counts as done — lives in `AGENTS.md`, not here.
