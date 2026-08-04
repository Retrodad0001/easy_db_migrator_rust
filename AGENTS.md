# Agents

Instructions for an agent working in this repository. What the project is, what
it is built from, and the rules the code itself follows live in
`ARCHITECTURE.md`; read that first. This file covers only how an agent is
expected to work here.

## Agent rules

- **Every quality check passes before a change is done** — after changing any
  code, the whole Quality checks section of `ARCHITECTURE.md` is run, not the
  subset that seems related to the change, and every gate comes back clean. A
  gate that was skipped, could not run, or has no tool installed is not a pass;
  it is reported as not run. A change is never called finished on a compile, on
  the tests alone, or on the assumption that an untouched gate stayed green. A
  gate that fails is either fixed or brought back to the user with what it
  actually said, and the change is reported as incomplete until then.
- **Quality check results are reported as a table** — one row per gate, covering
  every gate in the Quality checks section of `ARCHITECTURE.md`, with the outcome
  of each: passed, failed, or not run. Every gate appears every time, the passing
  ones included; a gate left out of the table reads as one that was quietly
  skipped. Prose instead of a table hides which gates were actually run, which is
  the thing the table exists to make impossible. Anything that failed is then
  spelled out below the table with what the tool actually reported, not a
  paraphrase of it. The shape:

  ```markdown
  | Gate | Result |
  |---|---|
  | Formatting | ✅ `cargo fmt --check` exit 0 |
  | No warnings | ✅ clippy `--all-targets -D warnings` clean |
  | Feature combinations | ✅ default, postgres-only, mssql-only all clean |
  | Tests | ✅ 18 passed, 0 failed, 0 ignored |
  | Integration container tests | ✅ mssql 9/9, postgres 9/9 |
  | Toolchain freshness | ⏭ weekly check already ran |
  | **Dependency advisories** | ❌ RUSTSEC-2026-0221, event-listener |
  | **Every behaviour and code path exercised** | ❌ `DbMigrator::new` |
  | | ❌ `SystemClock::now_utc` |
  | | ❌ delete-database failure branch |
  ```

  Two columns, `Gate` and `Result`. Each result cell opens with ✅ passed, ❌
  failed or ⏭ not run, then carries the evidence: the counts, the exit code, the
  advisory id. A bare tick is worth nothing — it reads the same whether the gate
  ran or was assumed. A failing gate's name is bolded so it is findable at a
  glance.

  Every gate in the Quality checks section gets its own row — all eighteen of
  them at the time of writing — named as that section names it and in the order
  it lists them. Gates are never collapsed into a shared row (`fmt / clippy /
  doc`) however alike their outcomes: the table is the checklist that proves each
  one was run, and a gate folded into a neighbour's row is a gate whose result
  nobody can point at. Counting the rows against the section is how the reader
  checks that nothing was quietly dropped, so the two must line up exactly.

  The **Every behaviour and code path exercised** gate names, in the result
  column, every path no test drives — each uncovered behaviour, function, match
  arm or error branch, listed out rather than counted. It takes one row per
  uncovered path: the gate name goes in the first row and the gate column is
  left empty on the rest, so six gaps read as six lines. Do not use `<br>` to
  break a line inside a cell — the terminal renderer prints the tag literally
  instead of breaking. A coverage gap that only
  appears in prose under the table, or as a bare ❌, is a gap someone has to go
  looking for. The proposed tests for those paths still go below the table, where
  there is room to say what each would set up and assert.
