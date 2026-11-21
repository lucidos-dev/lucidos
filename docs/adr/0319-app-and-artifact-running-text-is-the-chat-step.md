# 0319: App and HTML-artifact running text uses the chat prose step, sm; md is for labels and controls

- **Status**: Accepted
- **Date**: 2026-09-28

## Context

Apps and HTML artifacts written by the Lucidos Agent and by coding agents read
too big next to the chat that produced them. At a 125% UI scale, chat prose is
15px. An app's unsized text was 16.25px, and an unstyled HTML artifact was 20px.

Every app and artifact guide named `--font-size-md` the body default. The host
itself splits two roles: `md` for single-line UI (labels, rows, controls) and
`--font-size-sm` for running text (chat, file previews, settings descriptions).
The guides gave apps the UI step for prose, so an agent that followed them
exactly still wrote paragraphs a step above chat.

A standalone HTML artifact had no body default at all. It started at the
browser's 16px, and the preview then zoomed it by the UI scale (ADR 0217).

## Decision

Running text in an app or an HTML artifact uses the chat prose step, `sm`. `md`
stays the step for labels and controls. Two defaults enforce it for text that
names no size: the app iframe's `body`, and a zero-specificity `body` rule that
the artifact preview stamps.

## Rationale

An app or a report sits beside a chat reply in the same window. The eye compares
the two directly, so their paragraphs must share one step. The host already
answers which step that is.

The guidance was the root cause, so it changes first. The defaults are the net
for text an author never sized, the same role `base.css` plays in the host.

## Consequences

- An existing app's unsized text shrinks one step. Its explicit sizes are
  untouched, and its controls and shared row titles (`.list-row-info`) stay on
  `md`.
- An artifact's author rule on `body` still wins over the stamp, because the
  stamp uses `:where(body)`. An artifact that sizes only its root gets body
  text at `0.75rem` of that root, since `body` now carries a size of its own.
- The stamp sets no root font-size. An artifact that follows the type scale in
  `rem` renders exactly as before.
- The host's own `body` stays on `md`. Only the app and artifact contract moves.
- Every coding-agent session carries the scale's key facts in its prompt
  (`HTML_ARTIFACT_TYPE_SCALE_RULE`). A session that never loaded the skill
  wrote an HTML file at a 16px scale, so a skill alone is not enough.

## Alternatives considered

- **Guidance only.** Fixes future output, but leaves every unstyled artifact at
  20px and every unsized app paragraph a step high.
- **Stamp a root font-size on artifacts.** Would shrink an artifact's whole `rem`
  scale, so one that already follows the guidance would render a step too small.
- **Change the host's `body` to `sm`.** The host's single-line UI is
  deliberately on `md`. Changing it would resize the whole shell to fix a
  problem that only exists in apps and artifacts.
