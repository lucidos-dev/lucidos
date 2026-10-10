# 0282: A turn the engine resumes keeps the standing instruction of the turn it resumes: amends 0168

- **Status**: Accepted
- **Date**: 2026-09-25
- **Amends**: [0168: A thread acts in its own subtree](0168-a-thread-acts-in-its-own-subtree.md)

## Context

ADR 0168 lets a thread press the owner's buttons while it carries a standing
instruction, and a turn the owner opened is one. The check read the thread's
newest turn-start event. An engine resume writes one too: `ContinuationStarted`
after a version switch, an API error or a hang, with no device origin.

So the owner's own switch stripped their instruction from every turn it
resumed. A coding-agent thread then could not create a top-thread, and told the
user that Lucidos forbids it. It had run fine a few minutes earlier in the same
turn.

## Decision

A resume the owner did not click is transparent to the check. It reads the
turn start the resume continues, and weighs that exactly as before. The owner's
own Continue carries a device origin and still counts as a turn they opened.

## Rationale

**A resume is the same turn carrying on.** Clause 6 already says a turn opened
at 22:00 can act at 03:00. A restart in between changes nothing about who asked
for the work. The engine treats other continuations the same way: a parent
woken by `ChildThreadCompleted` writes no turn start and keeps its authority.

**It inherits and never promotes.** A resumed agent turn stays the agent's. A
resumed trigger fire is weighed by its trigger's provenance. A resume with no
turn behind it answers no.

## Consequences

- Apply, answering a card and creating a top-thread keep working across an
  engine switch, as they do across a long turn.
- The auto-resume gates (`switch_was_user_initiated`) are unchanged. This
  decides whose authority a resumed turn carries, not whether it resumes.

## Alternatives considered

- **Stamp the owner's device on the resume event.** Rejected. The owner did not
  press anything at resume time, and the route popover would say they did.
- **Keep the refusal and tell the agent to spawn a sub-thread.** Rejected. It
  gives the user a wrong answer about what Lucidos allows, and still breaks
  Apply and answering a card in the resumed turn.
