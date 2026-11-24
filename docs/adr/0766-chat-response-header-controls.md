# 0766: The response header always shows three evenly spaced icon controls and is not a click target

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0147](0147-a-turn-control-holds-what-you-pressed.md)

## Context

Each turn in the chat transcript has two headers, the initiator header and the
response header. Both carry controls that change how the transcript reads. Some
controls affect the whole transcript and one affects only its own turn. The
headers repeat down a long column, and the response header is the tightest row
in the app on a phone.

This record restates the design comments of
`crates/lucidos-app/src/components/chat/chat-exchange-parts.tsx`, written
before the comment strip removes them. Nobody checked it line by line against
the code.

## Decision

- The response header's `controls` slot holds three icon buttons right of the
  executor label, evenly spaced as one run (`.turn-controls`).
- The first two are the full response and the step log toggles. Both are on
  until the reader turns one off, and both always render, whatever the turn
  holds.
- The third is `collapseControl`, the same control the initiator header
  carries. It is the only one whose effect stops at its own turn, and its label
  says so.
- The two toggles sit on a pressed chip when on. The collapse control swaps
  minus for plus instead.
- The response header is not a click target. Folding a turn is the collapse
  control's job.
- The initiator header has one fold slot, right of the actor chip. It renders
  only where there is a body to fold.
- `canCollapseInitiator` in `ChatExchange` drops the fold for a user message
  and for a change turn.
- The response header's executor label is "Lucidos Agent", matching the
  initiator label used when one Lucidos thread spawns another.
- Claude Code reads "Claude" in that header and nowhere else. The icon and the
  executor popover carry the rest of the identity.

## Rationale

The two toggles flip a per-user setting that spans the whole transcript. A turn
with no steps is still a place to set how turns read. Rendering them
conditionally leaves holes in a column of identical headers.

The run stays evenly spaced because a 2+1 break reads as two things and a
stray, not as a scope split. The collapse label carries the scope instead.

A whole header row that silently swallowed a click announced nothing. It also
sat under three buttons that each mean something else. Both turn headers now
read that way.

The response header keeps a disabled control where one is dead. Two of its three controls are transcript-wide, and a hole in the run shows. The
initiator fold is omitted instead. Its panel body comes from its event rather
than streaming in, so a control that is dead now stays dead.

The coding agent's own name was the longest thing in the tightest row on a
phone. Every other "Claude Code" names the backend the user is choosing
between, so the full product name stays there.

## Consequences

- The response header looks the same on every turn, with the same three buttons
  in the same places.
- The two toggles appear on turns where they have nothing to show.
- Neither turn header responds to a click on its row. Only its buttons act.
- A user message and a change turn have no fold control.
- The label "Claude" appears only in the response header. The popover names the
  model exactly.

## Alternatives considered

- Rendering the two toggles only on turns that have steps. Rejected because it
  leaves holes in a column of identical headers.
- Breaking the run into a pair and a separate third control to show scope.
  Rejected because it reads as two things and a stray.
- Making the whole response header a click target that folds the turn. Rejected
  because a click it silently swallowed announced nothing, and it sat under
  three buttons with other meanings.
