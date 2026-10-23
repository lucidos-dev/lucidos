# 0370: Every side-question hold and the shortcut only turn on side-question mode; the Side question half is gone

- **Status**: Accepted
- **Date**: 2026-10-06
- **Amends**: [ADR 0347](0347-side-questions-have-no-slash-command.md), which kept the Send hold asking a draft at once

## Context

ADR 0347 added side-question mode for an empty box, and kept the older Send hold beside it. So the composer had two entry shapes, and each varied further by button and pointer:

- A hold on Send or Submit opened a "Side question" half, which asked the draft at once.
- A touch hold on Stop or a waiting card's Cancel opened the same half, whose tap turned on the mode instead.
- A mouse hold there, or ⌥↵, skipped the half and turned the mode on at once.
- The half took its button's shape and colour: a round blue pill, or a square one fused to green Submit or red Cancel.
- In the mode, a running turn asked with the round arrow, and a waiting card with a green square "Ask".
- A multi-select card and an idle empty box had no way in. ⌥↵ toasted "Type the side question first".

The user saw nine different states for one feature and asked for one. They picked this design from three rendered options. The renders and the plan are in `docs/plans/2026-10-06-side-question-entry-is-one-mode.md`.

## Decision

Every hold on the composer row's end button, and the Side question shortcut, only turn on side-question mode. That holds for every button, pointer and thread state. Any draft stays in the box. In the mode, a typed box asks with the round morph arrow. The Side question half is deleted.

## Rationale

1. **One gesture, one outcome.** The same press did different things depending on whether the box held text. Now it always does the same thing, and the pill says so.
2. **No gesture count is lost.** Asking a draft was hold, then tap the half. It is now hold, then tap Ask.
3. **One signal and one button.** The mode pill is the only side-question signal, and the round arrow the only Ask. A button whose shape and colour came from its neighbour is gone.
4. **The gaps close for free.** With nothing to ask yet, a mode is still meaningful, so the shortcut works over an idle box and a multi-select card.
5. **Less machinery.** The half was an overlay, a split-pill layout, a slide animation and a held exit drawing. None of it remains.

## Consequences

- On iOS a hold over an empty box cannot raise the keyboard, so the user taps the box to type. The half's tap existed to raise it. With the keyboard already up, a touch hold keeps it up.
- In the mode, an empty box keeps its thread-state button: Stop while a turn runs, the lone Cancel while a card waits. The turn stays stoppable.
- A second ⌥↵ turns the mode off, as × and Escape do. "⌥↵ then Enter" still asks a draft.
- The shortcut keeps its id, `askSideQuestion`, so a user's rebinding survives. Only its label changed.
- `SplitButton` takes optional press handlers for its primary face, so a multi-select card's Submit carries the hold.

## Alternatives considered

- **Keep the half, unify its look.** One round chip for every pointer and state, whose tap always turns on the mode. Rejected: it keeps the overlay and its animation, and adds a step where the hold could act directly.
- **Only fix the gaps.** Keep today's half, make the waiting-card Ask round, and let ⌥↵ work everywhere. Rejected: the two outcomes and the per-button shapes stay, which is what the user reported.
- **Keep the instant Send-hold ask beside the mode.** Rejected: it is exactly the one-gesture, two-outcomes split this ADR removes.
