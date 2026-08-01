# Agents

Instructions for an agent working in this repository. What the project is, what
it is built from, and the rules the code itself follows live in
`ARCHITECTURE.md`; read that first. This file covers only how an agent is
expected to work here.

## Agent rules

General agent rules for this repo live in the personal `jarvis` skill
(`~/.claude/skills/jarvis`) — never deleting `//TODO`/`//FIXME`/`//BUG` markers,
leaving it to the user to decide which tests get added, not changing or deleting
an existing test without asking, and not touching `Cargo.toml` without asking.
They are not restated here; a rule belongs in exactly one place. Where the two
conflict, this file wins.

- **Every quality check passes before a change is done** — after changing any
  code, the whole Quality checks section of `ARCHITECTURE.md` is run, not the
  subset that seems related to the change, and every gate comes back clean. A
  gate that was skipped, could not run, or has no tool installed is not a pass;
  it is reported as not run. A change is never called finished on a compile, on
  the tests alone, or on the assumption that an untouched gate stayed green. A
  gate that fails is either fixed or brought back to the user with what it
  actually said, and the change is reported as incomplete until then.
