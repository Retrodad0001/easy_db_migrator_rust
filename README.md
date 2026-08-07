# easy_db_migrator_rust

[![CI](https://github.com/Retrodad0001/easy_db_migrator_rust/actions/workflows/ci.yml/badge.svg)](https://github.com/Retrodad0001/easy_db_migrator_rust/actions/workflows/ci.yml)
[![CI Weekly](https://github.com/Retrodad0001/easy_db_migrator_rust/actions/workflows/ci_weekly.yml/badge.svg)](https://github.com/Retrodad0001/easy_db_migrator_rust/actions/workflows/ci_weekly.yml)

A lightweight, plain-SQL database migration library for PostgreSQL and
Microsoft SQL Server.

## What it is

A library crate — there is no CLI and no service. You call it from your
own application's startup path, or from a test.

It runs plain `.sql` files from a directory in the order their filenames
encode, records each one it applied in a tracking table, and skips
anything that table already lists. Re-running a migration is a true
no-op. There is no ORM and no migration DSL: what you write is the SQL
that runs.

It doubles as an integration-testing helper. A test points it at the
project's own migration scripts, builds the database the test needs,
runs against it, and drops it again afterwards. Both uses are
first-class.

## Status

Version 1.0.1. `Cargo.toml` sets `publish = false`, so the crate is not
on crates.io — take it by git URL or local path.

## Installing

Each backend is a Cargo feature, and you name the one you want:

| Feature | Backend | Driver |
| --- | --- | --- |
| `postgres` | PostgreSQL | `sqlx` |
| `mssql` | Microsoft SQL Server | `tiberius` |

```toml
[dependencies.easy_db_migrator_rust]
git = "https://github.com/Retrodad0001/easy_db_migrator_rust"
features = ["postgres"]
```

Nothing is enabled by default, so only the driver you named is compiled
into your build and you never take the other one by accident. There is
no `default-features = false` to write — there are no default features
to turn off.

Name both if you migrate both:

```toml
[dependencies.easy_db_migrator_rust]
git = "https://github.com/Retrodad0001/easy_db_migrator_rust"
features = ["postgres", "mssql"]
```

Naming neither is a build error: the crate stops on a `compile_error!`
rather than producing a `DatabaseKind` with no variants.

## Naming your scripts

Scripts are ordinary `.sql` files in a directory, named
`yyyyMMdd_NNN_name.sql`:

```text
migrations/
  20210926_001_AddEquipmentTable.sql
  20210926_002_AddIndexes.sql
  20211017_001_AddWorkoutTable.sql
```

They run ordered by date first, then by the three-digit sequence number
within that date. A filename that does not parse fails the run before
any script executes. Unlike the .NET version, nothing is embedded in the
binary — the directory is read at runtime.

## Applying migrations

Leave the database out of the connection string — the migrator selects
whichever database it is targeting itself, and creates that database and
the tracking table if they are not there yet. PostgreSQL takes a
`postgres://` URL; SQL Server takes an ADO-style connection string such
as `Server=tcp:localhost,1433;User Id=sa;Password=...;TrustServerCertificate=true`.

```rust
use std::path::PathBuf;

use easy_db_migrator_rust::{
    CancellationToken, DatabaseKind, DbMigrator, Error, MigrationConfiguration,
};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let config = MigrationConfiguration::new(
        "postgres://postgres:postgres@localhost:5432",
        "workout",
        PathBuf::from("migrations"),
    )?;

    let migrator = DbMigrator::new(Vec::<String>::new());

    migrator
        .try_apply_migrations(
            DatabaseKind::Postgresql,
            &config,
            &CancellationToken::new(false),
        )
        .await?;

    Ok(())
}
```

Swap `DatabaseKind::Postgresql` for `DatabaseKind::Mssql` to target SQL
Server; nothing else about the call changes.

The `Vec::<String>::new()` passed to `DbMigrator::new` is the list of
script filenames to exclude from this run. Name a script there and it is
left on disk untouched and never applied.

## Using it for integration testing

Delete the database first, so each test starts from a clean one:

```rust
let migrator = DbMigrator::new(Vec::<String>::new());

migrator
    .try_delete_database_if_exists(DatabaseKind::Postgresql, &config)
    .await?;

migrator
    .try_apply_migrations(
        DatabaseKind::Postgresql,
        &config,
        &CancellationToken::new(false),
    )
    .await?;
```

Give each test its own randomly named database and the suite can run in
parallel. `tests/postgres_integration_tests.rs` and
`tests/mssql_integration_tests.rs` do exactly this against real
containers, and are worth reading as worked examples.

`DbMigrator::with_clock_mock` takes a `ClockMock` in place of the system
clock, so a test can pin `executed_at` to a fixed instant and assert the
tracking rows exactly. It is not part of the supported API — application
code uses `DbMigrator::new`.

## What gets tracked

The migrator keeps one table, `DbMigrationsRun`, in the target database:

| Column | Meaning |
| --- | --- |
| `id` | Surrogate key |
| `executed_at` | When the script ran, in UTC |
| `filename` | The script's filename, its identity |
| `version` | Version of this crate that ran it |

A script whose filename already appears there is skipped.

## Cancelling a run

`CancellationToken` is checked between scripts, not during one, so a
cancelled run leaves the script it was in the middle of either fully
applied or not applied at all. Clones share one flag, so a clone moved
into another task cancels the run the original is driving. Cancelling is
not a failure: the run returns `Ok(())`.

## Errors and logging

Every fallible call returns a `Result` carrying this crate's single
`Error` type. When a run fails, the message on `Error::MigrationFailed`
is the same text the run reported
through [`tracing`](https://docs.rs/tracing), so the log and the error
never disagree. The crate emits `tracing` records rather than taking an
injected logger — subscribe to them however your application already
does.

## Alternatives

- [grate](https://github.com/erikbra/grate)
- [FluentMigrator](https://github.com/fluentmigrator/fluentmigrator)
- [EasyDbMigrator](https://github.com/Retrodad0001/EasyDbMigrator) — the
  .NET version of this migrator

## License

MIT. See [LICENSE](LICENSE).
