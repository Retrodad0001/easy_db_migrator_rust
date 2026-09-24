# easy_db_migrator_rust

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

The crate builds only inside its cargo workspace, `workspace_business`.
The workspace root holds the edition, the dependency versions, the lints
and the release profile, and this crate takes all four from it. A git
dependency on this repository alone does not build.

Each backend is a Cargo feature, and you name the one you want:

| Feature | Backend | Driver |
| --- | --- | --- |
| `postgres` | PostgreSQL | `sqlx` |
| `mssql` | Microsoft SQL Server | `tiberius` |

The workspace root names the crate once, by its path:

```toml
[workspace.dependencies]
easy_db_migrator_rust = { path = "easy_db_migrator_rust" }
```

A member of the workspace takes it from the root, with its backends:

```toml
[dependencies]
easy_db_migrator_rust = { workspace = true, features = ["postgres"] }
```

Name both if you migrate both: `features = ["postgres", "mssql"]`.

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
within that date. A file whose name does not match makes the run fail with
`ErrorKind::MigrationFailed`.

## Applying migrations

```rust
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use easy_db_migrator_rust::{
    DatabaseKind, ErrorKind, MigrationConfiguration, try_apply_migrations,
};

#[tokio::main]
async fn main() -> Result<(), ErrorKind> {
    let migration_configuration = MigrationConfiguration::new(
        "postgres://postgres:postgres@localhost:5432",
        "workout",
        PathBuf::from("migrations"),
        Vec::<String>::new(),
    )?;

    try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await?;

    Ok(())
}
```

`AtomicBool::new(false)` starts a run that is not canceled.

Swap `DatabaseKind::Postgresql` for `DatabaseKind::Mssql` to target SQL
Server.

The `Vec::<String>::new()` passed to `MigrationConfiguration::new` is the
list of script filenames to exclude from the run.

`MigrationConfiguration::new` returns `ErrorKind::InvalidConfig` when the
connection string is blank, or when the database name is empty or has more
than one word.

## Cancelling a run

Store `true` in the `AtomicBool` to stop a run before its next script. The
run reads the flag between scripts, so a script is either applied in full or
not at all. To cancel from another task, share the flag through an
`Arc<AtomicBool>` and pass a reference to it to the run.

## Using it for integration testing

Delete the database first, so each test starts from a clean one:

```rust
try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration)
    .await?;

try_apply_migrations(
    DatabaseKind::Postgresql,
    &migration_configuration,
    &AtomicBool::new(false),
)
.await?;
```

Give each test its own randomly named database and the suite can run in
parallel. `tests/integration_tests.rs` does exactly this against real
containers.

## What gets tracked

The migrator keeps one table, `DbMigrationsRun`, in the target database:

| Column | Meaning |
| --- | --- |
| `id` | Surrogate key |
| `executed_at` | When the script ran, in UTC, from the system clock |
| `filename` | The script's filename, its identity |
| `version` | Version of this crate that ran it |

## Errors

Every call that can fail returns `ErrorKind`:

| Variant | Meaning |
| --- | --- |
| `MigrationFailed` | A migration step failed, with the `tracing` text |
| `InvalidConfig` | The `MigrationConfiguration` is not valid |
| `InvalidScriptName` | `filename` does not match; `reason` says why |
| `ScriptsDirectory` | `path` could not be read; `source` is the I/O error |
| `Io` | A general I/O error, such as a failed TCP connection |
| `Postgres` | A PostgreSQL driver error, with the `postgres` feature |
| `Mssql` | A SQL Server driver error, with the `mssql` feature |

## Alternatives

- [grate](https://github.com/erikbra/grate)
- [FluentMigrator](https://github.com/fluentmigrator/fluentmigrator)
- [EasyDbMigrator](https://github.com/Retrodad0001/EasyDbMigrator) — the
  .NET version of this migrator

## License

MIT. See [LICENSE](LICENSE).
