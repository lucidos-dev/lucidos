# 0118: A UI surface owns its subscription; a pristine field follows the server

- **Status**: Accepted
- **Date**: 2026-08-24

## Context

Three `update_trigger` calls assigned three triggers to groups from a chat
thread. The trigger detail page was open. It kept showing the old group until
the page was reloaded.

Nothing in the pipeline was broken. The engine emitted `TriggerUpdated` from
inside the trigger write path, the frame reached the browser,
`processSSEForReferences` reloaded, and the `triggers` signal held the new
value. The page ignored it, because `TriggerFormInner` copies every field into
`useState` at mount and is keyed on the trigger id alone. A change to the same
trigger reuses the instance, and no initializer runs twice.

ADR 0032 made the emitting half mechanical: a state write owns its
announcement, enforced by `core/announced_surfaces.rs`. Nothing held the
receiving half. A surface could read state once and never hear about a change,
and no test noticed.

## Decision

Two rules, one for each half of the receiving side.

**A UI surface owns its subscription.** Every wire name in
`SystemEvent::RESERVED_TYPE_NAMES` is either handled by a frontend dispatcher
(`handleGlobalEvent` or `processSSEForReferences`) or listed once with a
reason. The reason table is `NO_UI_STATE` in
`store/actions/sse-event-coverage.test.ts`. That test fails on
anything else, so a new `SystemEvent` variant cannot land unanswered.

**A pristine field follows the server; a touched field keeps the draft.** A
form field over entity state uses `useServerBackedField`
(`hooks/useServerBackedField.ts`). Untouched, it returns the served value
itself and holds no copy. Touched, it holds the user's draft and ignores
frames. A setter call that lands back on the served value returns the field to
untouched.

## Rationale

The seam is not the transport. It is the copy. Seeding a `useState` from an
entity reads correctly and is wrong, because the initializer runs once per
mount while the entity keeps moving. Modelling pristine as *no copy at all*
makes the stale copy unrepresentable, which is stronger than remembering to
re-seed.

Refusing to overwrite a touched field is not an exception to the rule. Unsaved
work is not server state, so nothing about it is stale.

The event-coverage test is the frontend mirror of `announced_surfaces.rs`, and
for the same reason: a rule that lives only in prose is one forgotten call site
away from being false. It reads `RESERVED_TYPE_NAMES` out of the Rust source,
and `reserved_type_names_match_event_type` already guards that const against
enum drift, so the chain reaches the enum itself.

## Consequences

- An open detail page repaints from the frame, with no reload and no
  navigation. This is what the user asked for.
- A new `SystemEvent` variant costs one line: an arm, or a row in the reason
  table. The cost is deliberate, and it is paid once.
- An event that genuinely drives no UI state is written down as such. The next
  reader re-decides instead of re-discovering.
- A form over entity state now has one shape. `ThreadTitleEditor` and
  `EnvVarRow` already had it by hand, and both stay as they are.
- A field the user edits stops tracking the server for the rest of the
  session, unless they edit it back. That is the price of not losing work.

## Alternatives considered

**Re-key the form on the trigger's content.** A hash of the entity as the
`key` remounts the form on any change, which re-seeds every field for free. It
loses in-flight typing whenever anything moves, and `TriggerExecuted` moves
`last_run` on every scheduled fire. The user would lose a half-written intent
because a cron ticked.

**Re-seed from a `useEffect` on the entity.** Correct for a display-only field
and already used by `ThreadTitleEditor`, which re-seeds only while not
editing. As a general rule it still keeps a copy, so it needs an explicit
dirty flag per field, and a forgotten one fails silently. The hook makes the
same guarantee without the flag.

**Refetch on focus, on visibility, or on an interval.** Rejected outright. Each
makes the symptom go away on the reporter's machine and leaves the rule
unenforced, so the next surface repeats the bug. The engine already broadcasts
the exact frame that answers the question.

**Leave the enforcement in prose.** `.claude/rules/frontend.md` already binds
the emitting half in prose, and the emitting half is nonetheless enforced by a
test. The receiving half earns the same treatment for the same reason.
