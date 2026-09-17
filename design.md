# Design

## 1 UI DESIGN

`easy_db_migrator_rust` has no user interface. It is a library crate that a
program calls.

## 2 UI FLOW

`easy_db_migrator_rust` has no user interface. It is a library crate that a
program calls.

## 3 [STARTUP UI]

`easy_db_migrator_rust` has no user interface. It is a library crate that a
program calls.

## 4 STARTUP AND SHUTDOWN

### 5 Startup

`easy_db_migrator_rust` has no startup of its own. It is a library crate, and
the program that calls it starts it.

### 6 Shutdown

`easy_db_migrator_rust` has no shutdown of its own. It is a library crate, and
the program that calls it stops it.

## 7 PROGRAM DESIGN

- A migration is a plain SQL file. The library runs the file as it stands, and
  it holds no migration language of its own.
- A script file is named `yyyyMMdd_NNN_name.sql`. The date and the sequence
  number give the order, and a name that does not match is refused.
- Each database is a Cargo feature: `postgres` and `mssql`. There is no default
  feature, so a program takes only the driver it needs.
- The feature `dhat-heap` serves the performance test alone. With it, the profile
  program `examples/profile_migrations.rs` records the heap with `dhat`. The
  library itself does not read the feature.
- The public API is two functions: `try_delete_database_if_exists` and
  `try_apply_migrations`. Both take a checked `MigrationConfiguration`.
- The run creates the database and the tracking table `DbMigrationsRun` when
  they are missing, and it never deletes a database that it did not create.
- The tracking table holds the file name, the moment of the run and the version
  of the crate, so a second run skips a script that already ran.
- Each script runs in a transaction of its own. A failed script is rolled back,
  and every later script is skipped.
- The caller can stop a run with an `AtomicBool`. The library reads it before
  the run and between the scripts.
- The library reports through `tracing`, and it writes no log file of its own.
