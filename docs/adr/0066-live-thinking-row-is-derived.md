# 0066: The transcript's live Thinking row is derived at the end of each projection, never pushed from an event arm

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

The Lucidos Agent arm always owes the reader a live row: the engine emits
`ThoughtStreamed` before every LLM call, a `Thinking` row opens there, and the
call that pass produces renames it in place.

A coding agent emits nothing of the sort between a `CodingAgentToolResult` and
the next `CodingAgentToolCalled`. The transcript was therefore a column of
finished checks with nothing live in it. The only thing saying work was
happening was the "Working" label in the response header.

The same gap opens at the start of a resumed session, which fires no
`CodingAgentPromptSent` at all. For the 15 to 20 seconds before the first tool
call the turn holds only `SessionStarted` and two `CodingAgentSettingsChanged`,
none of which draws a row.

## Decision

Both transcript projections in
`crates/lucidos-app/src/store/thread-events/exchange-render.ts` DERIVE the live
`Thinking` row after their event loop has run, gated by `needsLiveThinkingRow`.
Neither pushes one from the `CodingAgentToolResult` or
`CodingAgentTextStreamed` arm.

## Rationale

The engine flushes coding-agent text at every renderable boundary
(`should_flush`: a paragraph break, a closed code fence, a heading, a rule), so
one multi-paragraph answer arrives as several visible `CodingAgentTextStreamed`
events.

A row pushed between them would defeat `mergeAdjacentTextEvents`, whose whole
job is to let a markdown document split across flushes render as one document.
A code block spanning two flushes would then render as two broken ones.

Appended last, the row cannot land between two text events. Derived only while
the turn is live, it leaves every finished transcript exactly as it was.

## Consequences

- The row is a pure function of the exchange, so the two projections cannot
  disagree about when one is owed.
- `needsLiveThinkingRow` has to restate the conditions its callers already
  stand in. Three of them are the branch it is called from, and are
  deliberately not re-tested: no terminator, thread not quiescent, and the turn
  not handed to a later exchange.
- The engine-down boundary must be excluded explicitly. A derived row is
  renderable, so it resurrects the response panel that
  `hasRenderableResponseContent` suppressed. The panel comes back with a
  "Working" badge over nothing but the dying subprocess's drain.

## Alternatives considered

**Push the row from the `CodingAgentToolResult` arm.** Rejected: it lands
between two text events of one flushed document and splits it, which is the
`mergeAdjacentTextEvents` breakage above.

**Push it from the `CodingAgentTextStreamed` arm instead.** Same defect, and
worse: every flush boundary is such an arm, so a long answer would grow a row
per paragraph.

**Leave the gap and rely on the header's "Working" label.** Rejected: the
header sits above a transcript that looks finished, and the reader looks at the
transcript.
