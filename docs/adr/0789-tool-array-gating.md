# 0789: Self curated context mode removes `todo_write` from the tool array instead of opening a gate

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0088](0088-tool-schemas-are-gated-by-capability-not-trimmed.md), [0109](0109-model-writes-notes-and-sees-its-own-context.md)

## Context

The tool array is built from gated families, as ADR 0088 sets out. Three gates
each open a family of tools for a workspace. Self-curated context mode, from
ADR 0109, does not fit that pattern.

A schema that is billed on every request and that nothing calls is a cost bug.
The mode exists to fix that cost.

This record restates the design comments of
`crates/lucidos-core/src/llm/tools/mod.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- Self-curated context mode is not a gate. It shapes the array and closes a
  family instead of opening one.
- The mode adds no tool. It takes `todo_write` away, which is otherwise offered
  to every workspace.
- The widest array, used by tests that ask whether a schema is registered at
  all, is every `Gate` open but `Gate::MemoryClassic`. It has `context_mode`
  off.
- Schemas the chat caller splices after the grouped manifest set live in their
  own table. `generate_image` is the entry that ADR 0088 extends.

## Rationale

- The checklist moved into the working understanding. A second write surface
  for the same list is the cost bug twice over.
- The two memory gates are one fact read two ways, so no workspace opens both.
  The widest array opens the Tree side and leaves `Gate::MemoryClassic` closed.
- The mode closes a family, so the widest array is the one with the mode off.
- The post-manifest schemas cannot live in `FAMILIES` without moving on the
  wire. A separate table of the same shape keeps `generate_image` in the one
  gate mechanism.

## Consequences

- A workspace in the mode is not offered `todo_write`, so no unused schema is
  billed on its requests.
- Tests that check whether a schema is registered use the widest array and do
  not depend on any one gate answering for them.
- Adding a post-manifest tool means adding an entry to the splice table. It
  does not mean adding a condition at the splice site.

## Alternatives considered

- Treat the mode as a gate. Rejected because it opens nothing and only closes a
  family.
- Keep `todo_write` alongside the working understanding. Rejected because two
  write surfaces for one list double the cost bug.
- Put the post-manifest schemas in `FAMILIES`. Rejected because that moves them
  on the wire.
- Gate `generate_image` with an `if` at the splice site. Rejected in favour of
  the one gate table.
