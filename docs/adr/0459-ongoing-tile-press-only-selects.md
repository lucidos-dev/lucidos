# 0459: An Ongoing tile press only selects its group; it never opens a thread

- **Status**: Accepted
- **Date**: 2026-10-12
- **Supersedes**: ADR 0401's "a press also opens the group's first thread", and
  its phone rule that opened a sole thread

## Context

A press on an Ongoing tile selected the group and also opened its first
thread. On the phone it opened only a group's sole thread. The user found the
jump confusing: the thread pane changed under them when all they asked for was
a different list. Opening a thread from the list is one more tap, which costs
little.

## Decision

A tile press selects its group, or steps the filter of a selected split group.
It never opens a thread, on any client and whatever the group holds. The
thread pane stays where it was.

## Rationale

- **One press, one effect.** A tile is a selector. A press that also navigates
  does two things, and the second one is easy to miss.
- **The same on every client.** The old rule opened on desktop, not on the
  phone, and on the phone only for a group of one. A reader could not predict
  which press would move them.
- **A tap is cheap.** The first row is right under the tiles, so the extra tap
  is short.

## Consequences

- Enter on a highlighted tile does what a click does: it selects.
- A Blocked or Review row still opens on what needs the user, as before.

## Alternatives considered

- **Keep the open on desktop only.** It still moves the thread pane on a
  press that only asked for a list, which is the confusion reported.
- **Open only a sole thread, on every client.** Same problem, and the reader
  cannot tell in advance whether a press will open something.
