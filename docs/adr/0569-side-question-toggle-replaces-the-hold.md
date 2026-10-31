# 0569: The composer has no hold: side questions start from a toggle or the shortcut, and every other long press fires while held

- **Status**: Accepted
- **Date**: 2026-10-24
- **Amends**: [ADR 0370](0370-side-question-entry-is-one-mode.md), which made every composer hold turn on side-question mode

## Context

ADR 0370 made every hold on the composer row's end button turn on side-question mode. On a phone that hold was the only entry. An iOS PWA sometimes sends a `touchstart` and then no lift at all. The 450 ms hold timer read that silence as a held finger, so a plain tap on Send opened a side question.

A first fix moved every long press in the app to fire at its lift. That cured the false hold, and it made every menu open on release. The user found that wrong: a long press should answer while the finger is still down.

## Decision

No composer button has a hold, on any device. Side questions start from a question-mark toggle in the composer row and from the shortcut (⌥↵), both taps. Every other long press fires on its timer while the finger is down: drawer rows, app rows and dashboard widget tiles.

## Rationale

1. **Remove the cause, not the symptom.** The false hold came from a hold on a button a user taps all day. With no hold there, a lost lift cannot start anything.
2. **The cost moves to where the gesture is wrong.** A lift ruling taxed every long press to protect one button. The ruling is gone, and the rest return to the feel users expect.
3. **One mode, three ways in and out.** The toggle, ⌥↵, the pill's × and Escape drive the same per-thread selection, so the toggle needs no state of its own.
4. **A visible entry beats a hidden gesture.** The toggle shows when a side question is possible and lights while the mode is on. A hold had no sign at all.

## Consequences

- The toggle taps through `keepsFocus` and focuses the composer inside the tap, so iOS raises or keeps the keyboard.
- `SplitButton` loses its primary press handlers, and the composer loses `sendHold` with its release marks.
- A lost lift on a long-pressable row can again fire its menu. The `deadPressProbe` keeps investigating the wedge itself.
- On a desktop, right-click on a composer button no longer turns on the mode.

## Alternatives considered

- **Keep the lift ruling.** Rejected: it fixed one button and slowed every other long press.
- **Keep the hold on a desktop only.** Rejected: two entry shapes was the state ADR 0370 removed.
- **Guard the hold against a lost lift.** Rejected: silence cannot tell a held finger from a lost one, which is why the hold left the composer.
