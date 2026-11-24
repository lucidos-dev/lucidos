# 0767: The four transcript event surfaces share one row look and one shared chip atom

- **Status**: Accepted
- **Date**: 2026-11-22

## Context

`EventRow.tsx` draws four kinds of event surface in the chat transcript. They
need to read as one family. Event types appear on several lines, including the
delivery card's subject. The test infra has no jsdom, so tests cannot render a
component that carries a hook.

This record restates the design comments of
`crates/lucidos-app/src/components/chat/EventRow.tsx`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- The row's look never branches on its kind. The kind is carried as `data-kind`
  for tests and kind-specific CSS only.
- The shared chip is the only way an event type is spelled anywhere in the
  transcript. It is exported, because a chip is not always a fact. The delivery
  card's subject is one.
- The facts line has two item kinds: `chip` and `glue`. `glue` is a connecting
  word such as "watching for" or "or", and the separator skips it on both
  sides.
- There is no `link` item. The chip carries the jump itself through
  `EventRowChip.onClick`.
- The row markup is a hookless pure function of its state. A caller that needs
  a hook owns it in a thin wrapper and passes the result down.
- The chip is a plain function, not a component, like `eventRowBody`.
- The chip shows the plain name as visible text, with a tooltip that says what
  it means. The raw type is not on the chip.
- With `onClick` the chip is a real `<button>`. Its accessible name says what
  pressing it does.

## Rationale

- Keeping the look independent of kind is what keeps the four surfaces
  coherent.
- One chip atom means one event type is never spelled two ways, whichever line
  of a card it lands on.
- The old `link` item was a "Go to event" next to the chip naming the very
  event it went to. That gave two things to read for one destination, and one
  of them repeated nothing.
- The raw type is still reachable elsewhere. A turn's route popover lists it
  under Technical details, with the types a re-entry's wait watched. A filtered
  watch's condition view names it too.
- A real `<button>` is reachable by keyboard and announces itself. A `<code>`
  carrying a handler does neither.
- The accessible name says what the press does, because the visible text says
  only what the event is.
- There is no jsdom in the test infra. A component carrying a hook cannot be
  invoked as a plain function.
- Tests walk the vnode tree that these functions return. A component vnode is
  opaque to that walk.

## Consequences

- Kind-specific styling can hang off `data-kind`, but the row's look must not
  depend on it.
- A new surface that names an event type uses the exported chip, not its own
  markup.
- A caller that needs a hook, such as the wait's jump tracking its in-flight
  click, adds a thin wrapper around the pure row.
- Tests call the row body and the chip directly and inspect the returned vnode
  tree.
- The chip's visible text does not show the raw event type.

## Alternatives considered

- A separate `link` item on the facts line, such as "Go to event". It is
  rejected because it duplicates the chip's destination.
- A `<code>` element carrying a click handler for the chip. It is rejected
  because it is not keyboard reachable and does not announce itself.
- A chip as a component rather than a plain function. It is rejected because
  tests cannot walk a component vnode.
- Hooks inside the row markup. They are rejected because tests cannot call such
  a component as a plain function.
