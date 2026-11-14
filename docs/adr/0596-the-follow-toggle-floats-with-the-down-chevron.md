# 0596: The follow toggle floats over the composer in the down chevron's old slot, always shown; the chevron stands one slot above it

- **Status**: Accepted
- **Date**: 2026-10-28

## Context

A reader who wants the bottom of a thread met two controls in two places. The
round down chevron floated over the transcript. The follow toggle (the magnet)
stood first in the composer row. The maintainer asked to bring them together.

ADR 0064 keeps them as two controls: the magnet is the standing follow, the
chevron the one-shot jump. So the question was placement only. Rendered
variants were compared in the running app, and the plan is
`docs/plans/2026-10-28-the-follow-toggle-floats-with-the-down-chevron.md`.

## Decision

The magnet leaves the composer row. It floats over the composer's top edge in
the down chevron's old slot, and it is always shown, compose view included. The
chevron stands one slot higher and fades in while the reader is scrolled up.

## Rationale

- **The magnet never moves.** It holds one fixed spot, so nothing slides under
  the pointer after a chevron press lands the reader at the bottom.
- **It is anchored to the composer, not the transcript.** It rides with a
  growing prompt, the pills over the prompt box and the phone's dynamic bars.
  It also stands in the compose view, where ADR 0064 says the toggle matters
  most and where there is no transcript or chevron.
- **The two read as one family.** Both wear the chevrons' round surface, in one
  vertical stack, and the stack order says which is which: the jump is above,
  the standing state below.
- **The composer row gets a slot back.** That slot goes to the members that fold
  first on a narrow screen.

## Consequences

- `FollowLiveEdgeToggle` (`components/chat/FollowToggle.tsx`) renders inside
  `.prompt-area`, beside `PromptInput`. The row's spec and the keyboard
  shortcut share `toggleFollowLiveEdge`.
- The Diff and standing-apply merge keys on `mergeAt`, the row's first member
  after the readouts, since the magnet no longer anchors it.
- The transcript keeps a constant bottom reserve, `--transcript-end-reserve`,
  that clears the magnet at the live edge. It never reads the follow flag
  (ADR 0064). The slot geometry lives in `--scroll-chevron-inset` and
  `--scroll-chevron-stack-gap`.
- On touch the stack gap is one tap reach, and the two buttons each reach half
  of it toward the other. That is the tightest gap that keeps the down
  chevron's target at 44px. The maintainer found two reaches too far apart.
- Under the phone's dynamic bars the magnet shifts with the prompt, so it stays
  one slot under the chevron when the prompt hides.

## Alternatives considered

**A corner button inside the prompt box.** It costs the textarea its right edge
on every line, which `composer-single-right-anchor.test.ts` already records as
a rejected shape.

**The chevron joins the composer row.** The chevron only makes sense away from
the bottom. In the row it would either hide and shift its neighbours, or stand
disabled most of the time.

**The two side by side.** Two round buttons in a row read as one choice between
two options. They also cover twice the width of passing content.

**The magnet on top, the chevron below.** The magnet shows at the live edge. In
the top slot it needs about three lines of bottom padding to clear the last
turn. The bottom slot needs about one.

**One button for both jobs.** Rejected by ADR 0064 § Alternatives considered:
go-to-bottom, arm and disarm make a three-step cycle with no state left over for
a plain jump.
