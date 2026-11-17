# 0657: The People row folds into a title-row icon with a rolling panel, and the agent pause shrinks to a composer placeholder: amends ADR 0649

- **Status**: Accepted
- **Date**: 2026-11-11

## Context

ADR 0649 gave a team thread three standing surfaces: a People row under the
header, a strip above the composer ("Agent paused. Mention @agent or Resume"),
and a card in the transcript for every pause and resume. On a busy team thread
all three draw at once. The thread reads as noisy, and none of them is the
thing the user came to read.

## Decision

1. **The People row becomes one icon in the title row.** It shows the driver's
   mark and a chevron. Tapping it rolls a panel open with a row per person,
   Watch or Unwatch, Take over or Release, and an "Edit people" link. This
   amends decision 6 of ADR 0649.
2. **The pause is a composer placeholder.** While the agent is paused, the
   empty composer reads "Agent paused. Mention @agent to wake it". A side
   question or an answer to a question card still wins the placeholder.
   Nothing else draws. This amends decision 8 of ADR 0649.
3. **The strip, its Resume button and the pause cards are gone.** `AgentPaused`
   and `AgentResumed` open no exchange, draw nothing and never split a running
   turn. A message mentioning @agent ends the pause, as before.
4. **The engine keeps its pause route and events.** Only the frontend draws
   less.

## Rationale

The pause is a state of the composer, so the composer is where it belongs: the
user sees it exactly when they are about to type. A people-only message already
reads as a bubble without a marker, and the placeholder tells the reader why.
People are an occasional concern, so a closed icon is enough until the user
asks.

## Consequences

- A paused thread has no Resume button. Mentioning @agent is the one way back,
  which matches what the placeholder says.
- The pause leaves no mark in the transcript. The events stay in the log.
- The thread header gains no new row, so a thread with people looks like any
  other until the icon is opened.

## Alternatives considered

- **Keep the strip, shrink it.** It still sat between the transcript and the
  composer on every paused thread.
- **A pause marker on each people-only message.** Rejected in ADR 0649 and
  still noisier than the placeholder.
- **Keep the pause cards as quiet transcript rows.** The placeholder already
  says the state. A second record only repeats it.
