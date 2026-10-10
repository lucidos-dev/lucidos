# 0221: The composer rests at one line, and declares no multi-line floor

- **Status**: Accepted
- **Date**: 2026-09-19

## Context

The chat composer shares the `.prompt-box` shell with the trigger form's Intent
field. The shell floors an empty textarea at a line plus its own padding:
`2.75rem` on a desktop pane, `2.25rem` under the mobile breakpoint. A follow-up
therefore goes in a box the size of one line, which reads as small beside the
turn it docks under.

A three-line floor was added on top of that shell, `min-height: calc(3lh + 1rem)`
scoped by `.prompt-row`, to make the box look like somewhere to write. It was
reported the same day, first on a phone and then on a desktop pane.

## Decision

The composer declares no resting height of its own. It rests at the shared
shell's floor on both breakpoints and grows on typing, which `resizeTextarea`
already does.

## Rationale

An empty box is not where the writing happens, so height spent in advance is
height spent on nothing. On a phone it is not free either: three empty lines
took about a tenth of the screen. They took it from the transcript the composer
docks over, on the surface with least room to give.

Growth on typing already covers the case the floor was reaching for. The box
follows the text from the first character, so a long follow-up gets its room
when it exists rather than before it.

## Consequences

- One resting height per breakpoint, and it comes from one place: the shell.
- The cascade scan that pinned the composer's own floor is gone with the floor.
- `e2e/composer-resting-height.spec.ts` keeps the claim, now bounded against
  the element's own line-height, so a re-added multi-line floor fails it on
  every project.
- The answered-question bound in `e2e/coding-agent-question.spec.ts` is back to
  two lines, one line of tolerance over resting.

## Alternatives considered

**A one-line floor of the composer's own** (`calc(1lh + 1rem)`, in `lh` so it
tracks the type scale). Shipped for mobile alone and then dropped. It lands
within a fifth of a pixel of the shell's `2.25rem`. So it bought a rule the
cascade scan had to keep honest, and changed nothing on screen.

**Two lines.** Offered beside one line when this was reverted the first time,
and not chosen. It is the same bet as three, at a smaller stake: room reserved
for text that is not there yet.

**A taller floor on desktop only**, where the pane has the room. This is where
the change stood for a few hours, and the next report was against the desktop
pane. Screen area is not the argument: an empty box is empty at any width.
