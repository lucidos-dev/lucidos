# 0166: The frontend's ThreadEvent payload types are generated from the Rust source by a syn reader, not hand-mirrored and not by a derive macro

- **Status**: Accepted
- **Date**: 2026-08-30

## Context

Rust is the source of truth for event types (CLAUDE.md, Core Architectural
Principles). Only event NAMES reached TypeScript by generation. Every payload
FIELD was hand-mirrored in `store/thread-events/thread-event-types.ts`, so a new
Rust variant was loud and a new Rust FIELD was silent.

Measured against the enum on the day this landed, the hand-written union carried
15 Rust fields with no TypeScript counterpart. Among them were
`ToolResult.success`, `ChangeProposed.branch_name` / `repo_root` / `hardened`,
and `CodingAgentIdled.worktree_path`. It also declared four fields nothing
emits, two of them as REQUIRED booleans that were always `undefined` at runtime.
`system-knowhow/thread-events.md` described all 15 correctly, so the LLM-facing
prose was more accurate about the payload than the type the frontend compiled
against.

The immediate trigger was a change adding `modality` to `ApiUsage`, which had to
hand-edit two separate TypeScript spellings of the same block. Two object types
that differ by one optional field are assignable both ways. So `tsc` stays
silent when one copy is updated and the other is not.

## Decision

A test-only `syn` reader (`thread_events_tests/ts_codegen.rs`) parses the
`ThreadEvent` enum and its 26 payload types out of the engine source and emits
`crates/lucidos-app/src/generated/thread-event-wire.ts`. It follows the repo's
existing codegen shape: an `#[ignore]` writer test paired with a staleness test.

Generation covers the **wire shape** only:

```
wire = ThreadEvent variant  +  EventMeta fields  +  API stamps  -  API strips
```

View models in `store/types.ts` stay hand-written. What changed for them is that
they re-export the generated payload types instead of re-spelling them.

## Rationale

**A derive macro cannot read the attribute that decides optionality here.** The
enum carries 130 `skip_serializing_if` and 152 `serde(default)` attributes. Both
mean the key can be absent on the wire, one on a new row and one on an old one.
So both must produce an optional TypeScript property.

`ts-rs` and `specta` express that with a per-field `#[ts(optional)]`. That
annotation would land on roughly 150 production fields. It can then drift from
the serde attribute beside it, which is the same failure class this change
removes.

**Two divergences cannot be expressed on the Rust type at all.**
`ContextCaptured.sections` is required in Rust and optional on the wire, because
the snapshot endpoint strips it for size. A derive would need an annotation on a
production field for a reason that has nothing to do with Rust. The reader keeps
those in a declared table instead, where a reviewer can see the whole list.

**The divergence list is small enough to declare.** Three retired variants, one
retired enum arm, three legacy fields, three strips, two stamps, three merged
meta fields and three widened field types. Twenty declared rows replace 79
hand-maintained union members.

**The reader refuses rather than guessing.** An unregistered payload type fails
the generator by name, so a new supporting type cannot reach the wire
undescribed. A Rust doc comment carrying an em dash or an ISO date fails it too.
The generated file ships, and it is scanned like any hand-written source.
Laundering a rule violation into it would fail a gate with no author to point
at.

**It cost no new dependency.** `syn` was already in `Cargo.lock` and in the
local registry cache. Enabling its `full` feature as a dev-dependency downloads
nothing and adds nothing to the shipped binary.

## Consequences

- A new Rust payload field reaches TypeScript by regenerating, and the staleness
  test fails until someone does. A new supporting type fails the generator until
  it is registered.
- Rust doc comments are carried across as TSDoc, first paragraph only. Depth
  stays in the Rust source, which the generated header names. A note that
  matters to a frontend reader belongs in the Rust doc, and several moved there
  in this change.
- Two hand-maintained hops closed. `all_persisted_event_types()` is a hardcoded
  list that nothing guarded; it is now asserted against the parsed enum. And
  `PersistScope` was renamed `AllowScope` after the Rust enum it mirrors.
- The generated file is ~2,100 lines against the 914 it replaces, because every
  field now carries its Rust prose. Nobody reads a generated file, and the
  hand-written module shrank to 330 lines of genuinely frontend-only logic.
- `.claude/rules/testing.md` previously recorded the opposite decision. It is
  rewritten in the same change.

## Alternatives considered

**`ts-rs` or `specta` derives on the production types.** Rejected on the
optionality point above, on the two inexpressible divergences, and because it
needs a new dependency reaching 26 production types across 10 files. It would
have won on one count: it follows the type graph automatically, where the reader
needs a declared registry. That registry turned out to be an asset, since it is
what makes an unregistered type a hard error.

**A field-level drift guard with no generation.** Emit a machine-readable field
contract from Rust and fail a test when a Rust field has no TypeScript
counterpart. Rejected because the expensive part is the reader, which both
options need, so generating costs little more than diffing. A guard also reports
the 15 missing fields and then leaves a human to hand-write them correctly. It
cannot check a field whose type is wrong.

**Generating the view models too.** Rejected. `ContextCapture` and `Step` add
frontend-only fields on purpose and are not mirrors of anything. Generating them
would need a second divergence table larger than the types themselves.

**Generating `system-knowhow/thread-events.md`.** Rejected. It is prose about
when and why each event fires, not a machine mirror. Generating it would lose
exactly what makes it useful to the workspace LLM.
