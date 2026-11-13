# 0332: An em dash with a space on both sides is allowed; the unspaced form and U+2015 stay banned

- **Status**: Accepted
- **Date**: 2026-09-30

## Context

`.claude/rules/em-dashes.md` banned U+2014 outright. The ban held in files and
commits, where two hooks enforce it. It did not hold in chat, where only the
model's own compliance stands between the model and the user.

A nightly e2e thread on Opus 5.5 wrote `start<U+2014>another` in a chat reply,
with the ban in context twice. The theme font is monospace. It draws an em dash
one cell wide, the same width as a hyphen, so the reply read as
`start-another`. A ban that the model still breaks in chat produced the worst
rendering on offer.

## Decision

An em dash with whitespace or a line edge on both sides is allowed. An unspaced
em dash stays banned, and U+2015 HORIZONTAL BAR stays banned in every form. The
chat renderer adds visual space around an unspaced dash that slips through.

## Rationale

- **Spacing, not the character, is what reads wrong.** Unspaced is correct
  Chicago style in a proportional font. AP style spaces the dash, and a spaced
  dash reads as a break in any font, monospace included.
- **A total ban does not survive chat.** No hook sees a chat reply. When the
  model slips, it writes its natural form, which is unspaced.
- **The gates stay deterministic.** "Touching a non-whitespace character" is a
  byte check with no judgment in it, so the hook and the harden check agree.
- **The display fix covers what the rule cannot.** Slips, pasted text and old
  messages all render with a visible break. It is a styled span, so copied
  text keeps the author's bytes.

## Consequences

- The ~29,000 existing lines are mostly unspaced and stay as they are. Touching
  one still means fixing it.
- An agent may now write a spaced dash. The rule still steers toward a comma,
  colon, parentheses or a new sentence first.
- The rule file and hook lost their `no-` prefix, since the name no longer
  described them.
- A user's own global instructions may still ban em dashes. Those win for that
  user, and nothing here overrides them.

## Alternatives considered

- **Keep the total ban and fix only the display.** It leaves the rule
  contradicting what the model does in chat, and every slip still breaks it.
  The display fix is part of this decision anyway.
- **Lift the ban entirely.** Unspaced dashes would return to files, where the
  same monospace rendering applies in editors and diffs.
- **Make the spaced en dash the approved break (British style).** It looks
  almost like a spaced em dash, so it gains nothing visible. It also overloads
  a character this rule leaves for numeric ranges.
- **Insert real spaces into chat text at render time.** It would change what
  the user copies, so the renderer uses a span with margin instead.
