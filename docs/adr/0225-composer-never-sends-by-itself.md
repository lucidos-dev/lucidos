# 0225: The composer never sends by itself: the dead-press commit is retired

- **Status**: Accepted
- **Date**: 2026-09-19

## Context

[ADR 0183](0183-a-dead-composer-tap-runs-the-commit-face.md) gave a stationary
tap that reached the composer row and no face two answers. It ran the commit
face, then relaid out the shell. The sixteenth report was the first confirmed
fire and it was right: the press landed 8 px from Send, the draft went, and the
reporter chose to keep both halves.

The seventeenth report is the same mechanism getting it wrong. A tap landed on
bare row space at (191, 414), 138 px from a live Send. The settle clicked Send
and the draft went out. In the reporter's words, "it sent when I hadn't tapped,
that must not happen".

Nothing in ADR 0183's five bounds could tell 138 px from 8 px. Travel, a claim,
a cover, a live commit face and a re-read at firing are all cleared by a
deliberate tap on empty row space. The distance was measured on the neighbouring
`missed` line as `missedBy.px`, and then thrown away.

The ledger says how narrow the escape had been. It holds 41 presses that reached
the row and no face. Three had a live commit face, which is the only state the
settle can fire in. One was refused as `claimed`, and the other two fired at
8 px and at 138 px. The remaining 38 sit between 58 px and 231 px, clustered in
the middle of the row. Every one of them would have sent a draft had a commit
face been live.

## Decision

**The composer never commits on its own.** The Send half of ADR 0183 is
reversed. `deadPressProbe.ts` dispatches no click on any element on any path,
and a message goes when the user presses Send and at no other time.

**Where the press landed IS a bound now**, and it survives the retirement. ADR
0183 refused one on the reporter's finding that the composer is dead wherever
the finger lands. That finding is about where the DEADNESS is. The bound is
about which ACTION the user meant, and conflating the two is what sent the
draft.

So the settle keeps every bound it had, adds a reach bound at 12 px, and decides
what the LINE says rather than what runs. A press that clears every bound writes
`commit-withheld`. One that does not writes `rescue-stood-down` naming which
bound refused it, `off-face` included. Both carry `reachPx`, so a fire is
auditable on its own line instead of on its neighbour.

**A press that shared the glass is not dead.** WebKit synthesises no click for a
multi-finger gesture, so such a press produces exactly what a dead one does. The
same report opened with an Archive press called dead, whose `quiet.ms` of 13
places a second contact 13 ms in front of it. Every press line now counts the
contacts, a shared press reads `multi-touch`, and no report toasts on one.

Kept from ADR 0183, unchanged: the relayout and its downward bounce, the
typing-driven trigger, every instrument on the ledger, and the 600 ms grace
window. Return-to-send on mobile stays rejected.

## Rationale

A diagnostic that acts can be wrong in a way a diagnostic that reads cannot. The
worst a wrong reading costs is a confusing line in a log. The worst a wrong
action costs is a message sent to somebody, which is unrecoverable, and it
happened.

The recovery that needs no intent is the relayout, and it stays. It commits
nothing, the user sees nothing, and it is the half the ledger has scored twice.
The commit was the half that needed to be right about intent, and it had no way
to be.

The reach bound is kept even though nothing runs, because a line is worth
reading only where the press was aimed at the face. Without it, every tap on
empty row space would claim a dead commit. The 38 such taps in one day's ledger
would then drown the three that matter.

## Consequences

The wedge now costs the user a second tap again, as it did before ADR 0183. That
is the trade, and it is the right way round: a tap they have to repeat is a
nuisance, and a message they did not send is not.

The investigation keeps everything it had. A run of `commit-withheld` lines at a
`reachPx` of zero is what would justify a bounded commit coming back. Nothing
could produce that reading before. A run of `off-face` lines beside them says
the opposite.

`activated` leaves the verdict list and `commit-withheld` and `multi-touch` join
it. An earlier ledger still reads: no existing verdict changed meaning.

The `Archive` button keeps its click-only path. It was called dead in the same
hour, and on this reading the platform was working. Giving it a touch path is a
separate decision on separate evidence.

## Alternatives considered

**Bound the commit to the face and keep it.** The plan's first draft, and what
the user was offered. It refuses 138 px and keeps the 8 px fire that worked. It
lost because it leaves the app able to send a message on evidence it cannot
fully check: a displaced hit test moves the reported point, so a press that
reads as on the face need not be one. The user chose the retirement and asked
for the bound as well.

**Keep the commit and add an undo window.** A toast with an Undo, or a delayed
send. It turns one unrecoverable action into two surfaces to get right, and it
still sends by default. A message the user did not ask for is not fixed by
offering to take it back.

**Retire the whole probe.** The wedge is unfixed and no emulator reproduces it,
so the ledger is the only instrument there is. Retiring it would end the
investigation to fix a defect in one branch of it.

**Toast on every dead tap instead.** Rejected in ADR 0183 and still rejected.
A tap on empty row space is not a fault, and 38 toasts a day teaches the reader
to ignore the one that matters. The `landingReport` toast survives because it
already requires the finger to be inside a face's painted box.
