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
