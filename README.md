# easy_db_migrator_rust

[![CI](https://github.com/Retrodad0001/easy_db_migrator_rust/actions/workflows/ci.yml/badge.svg)](https://github.com/Retrodad0001/easy_db_migrator_rust/actions/workflows/ci.yml)

A lightweight, plain-SQL database migration library for PostgreSQL and
Microsoft SQL Server.

## What it is

A library crate — there is no CLI and no service. You call it from your
own application's startup path, or from a test.

It runs plain `.sql` files from a directory in the order their filenames
encode, records each one it applied in a tracking table, and skips
anything that table already lists.

It doubles as an integration-testing helper. A test points it at the
project's own migration scripts, builds the database the test needs,
runs against it, and drops it again afterwards.

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

Name both if you migrate both:

```toml
[dependencies.easy_db_migrator_rust]
git = "https://github.com/Retrodad0001/easy_db_migrator_rust"
features = ["postgres", "mssql"]
```

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
within that date.

`Script::parse` is the same parser the migrator uses, exposed so a build step
or a test can check a filename without running anything. It returns a `Script`
whose `filename`, `content`, `date_part` and `sequence_part` are the pieces it
read, or an `Error` naming what was wrong with the name.

## Applying migrations

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

`CancellationToken::new(false)` starts a run that is not cancelled.

Swap `DatabaseKind::Postgresql` for `DatabaseKind::Mssql` to target SQL
Server.

The `Vec::<String>::new()` passed to `DbMigrator::new` is the list of
script filenames to exclude from this run.

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
containers.

## What gets tracked

The migrator keeps one table, `DbMigrationsRun`, in the target database:

| Column | Meaning |
| --- | --- |
| `id` | Surrogate key |
| `executed_at` | When the script ran, in UTC |
| `filename` | The script's filename, its identity |
| `version` | Version of this crate that ran it |

## Alternatives

- [grate](https://github.com/erikbra/grate)
- [FluentMigrator](https://github.com/fluentmigrator/fluentmigrator)
- [EasyDbMigrator](https://github.com/Retrodad0001/EasyDbMigrator) — the
  .NET version of this migrator

## License

MIT. See [LICENSE](LICENSE).
