/**
 * Pure rendering utility functions for Exchange / ResponseEvent display.
 * No side effects, no signals — safe to import from components and tests.
 */
import type { ResponseEvent } from './types';

/** True when a streamed text chunk puts something on screen.
 *
 *  Blank chunks are not a curiosity, they are the norm on a coding-agent
 *  thread: every `CodingAgentToolCalled` is preceded by a whitespace-only
 *  `CodingAgentTextStreamed`. The renderer already drops them (`ChatExchange`
 *  skips a text event with no `md.trim()`), so anything that treats a text
 *  chunk as "the model produced output" has to ask this first, or it acts on
 *  output the user never sees. */
export function hasVisibleText(text: string | undefined): boolean {
  return !!text && text.trim().length > 0;
}

/** True when `e` is a text event whose markdown is non-empty after trimming. */
export function isMeaningfulText(e: ResponseEvent): boolean {
  return e.type === 'text' && hasVisibleText(e.md);
}

/** True when an in-progress step is actually rendered on screen — steps are
 *  expanded, the response panel is NOT collapsed, AND the visible set holds a
 *  pending step (`outcome === 'pending'`, the one that carries the
 *  `.running-shimmer` "live" affordance). Only `'pending'` counts, and the
 *  question is what SHIMMERS rather than what is terminal. `'unfinished'` is
 *  terminal and does not. `'blocked'` is not terminal and still does not: a
 *  call held on a permission card is waiting for the reader, so animating it
 *  would claim the machine is busy.
 *
 *  Drives the "exactly one running-text shimmer at a time" rule: when a live
 *  step is on screen its own shimmer is the live signal, so the status label
 *  ("Working") stays plain; otherwise the label itself shimmers as the sole
 *  affordance. The `collapsed` guard is load-bearing — a collapsed panel hides
 *  the steps body entirely (only the header status shows), so a pending step in
 *  the data is NOT on screen; without this, the label shimmer was suppressed
 *  while the step shimmer was hidden, leaving a working turn with no shimmer. */
export function hasVisibleLiveStep(showSteps: boolean, collapsed: boolean, visibleEvents: ResponseEvent[]): boolean {
  return showSteps && !collapsed && visibleEvents.some(e => e.type === 'step' && e.outcome === 'pending');
}

/** True when a rendered row is step **mechanics**: one line of the tool-by-tool
 *  log of how a turn did its work. Everything else a turn emits is a *transcript
 *  marker* (`docs/glossary.md`): a section break, a generated image, a command
 *  checkpoint with its Undo, an *event wait* row, the empty-response note. Each
 *  records that a thing happened at a point in the transcript, rather than a
 *  detail of how.
 *
 *  The distinction is what the two hiding mechanisms are FOR, so both read it
 *  here rather than each carrying its own list. The steps control hides
 *  mechanics; the collapsed (answer-only) view drops mechanics and earlier
 *  prose. Hiding a marker is a different thing entirely: the fact goes
 *  with it, and none of these is reachable anywhere else once it is gone.
 *
 *  The event-wait row is the case that proved it. It was classed as a step, so
 *  it was hidden by a default-off toggle AND dropped by a collapse that kept
 *  only two of the markers, and a parked thread showed no evidence anywhere in
 *  the transcript that it had parked. */
export function isStepMechanics(event: ResponseEvent): event is Extract<ResponseEvent, { type: 'step' }> {
  return event.type === 'step';
}

/** Does this event put a row in the response body RIGHT NOW?
 *
 *  An ALLOW-list, mirroring `renderResponseEvents` in `ChatExchange.tsx` arm
 *  for arm, because that renderer draws five of the nine event kinds and falls
 *  through to `null` for the rest. A deny-list ("anything that isn't a blank
 *  text") reads as the safe direction and is not: `question` and `permission`
 *  land in an exchange's events and draw nothing HERE, since they render as
 *  initiator-panel dividers with their own fold (`collapsedInitiators`), and a
 *  `section_break` is consumed by `splitEventSections`.
 *
 *  Of the five that do draw, two are conditional: a `text` needs visible text
 *  (one is pushed for every `CodingAgentTextStreamed`, and a torn-down
 *  subprocess signs off with a bare `"\n\n"`), and step mechanics need the
 *  steps control to be on.
 *
 *  Deliberately NOT `hasRenderableResponseContent`, which is the same shape of
 *  question asked for a different purpose and reaches a different answer about
 *  a step. That one decides whether a turn is worth a PANEL, so it counts a
 *  hidden step: the header carries the control that reveals it, and a panel you
 *  can open is not a dead end. This one decides whether there is anything to
 *  FOLD, and a fold swaps the body for a `⋯` stub, so on a turn drawing nothing
 *  it swaps nothing for a mark, which is the opposite of collapsing. The two
 *  must not be merged; they share `isMeaningfulText` and `isStepMechanics` so
 *  they cannot drift on what those mean.
 *
 *  The switch is EXHAUSTIVE, with no `default`, which is what makes the mirror
 *  hold: `isStepMechanics` narrows the union, so a tenth event kind added to
 *  `ResponseEvent` fails to compile here until someone says which way it draws.
 *  A `default: return false` would take the new kind silently, and a kind that
 *  draws would then leave its turn permanently unfoldable. */
export function drawsResponseRow(event: ResponseEvent, showSteps: boolean): boolean {
  if (isStepMechanics(event)) return showSteps;
  switch (event.type) {
    case 'text': return isMeaningfulText(event);
    case 'image':
    case 'checkpoint':
    case 'event_wait':
    case 'spoken_reply':
    case 'spoken_message':
    case 'empty':
      return true;
    // Drawn somewhere else, or not at all. A question and a permission are
    // initiator-panel dividers with their own fold; a `section_break` is
    // consumed by `splitEventSections` and skipped by the renderer.
    case 'question':
    case 'permission':
    case 'section_break':
      return false;
  }
}

/** True when this exchange has prose that turning the full-response control OFF
 *  drops: `getCollapsedVisibleEvents` then keeps only what follows the last
 *  text block, so anything said earlier disappears.
 *
 *  Takes steps AND two or more meaningful text chunks. Steps because a chat
 *  turn that simply wrote two paragraphs has no superseded prose, only one
 *  answer; two chunks because with one there is nothing before the last.
 *
 *  This decides how the body RENDERS, never whether the control is offered.
 *  The two used to be the same predicate (the pair of text links appeared only
 *  where they had something to do), and they were split when the controls
 *  became fixed header icons: what they toggle is a per-user setting that
 *  spans the transcript, so a turn with nothing of its own to reveal still
 *  shows them rather than leaving a hole where its neighbours have controls. */
export function hidesEarlierProse(events: ResponseEvent[]): boolean {
  const hasSteps = events.some(isStepMechanics);
  const meaningfulTextCount = events.filter(isMeaningfulText).length;
  return hasSteps && meaningfulTextCount >= 2;
}

/** Determine which events are visible when the exchange is collapsed.
 *  Keeps events from the last text block onwards, plus every marker before it
 *  (see `isStepMechanics`): the collapse drops the mechanics and the superseded
 *  prose, never the record that something happened. */
export function getCollapsedVisibleEvents(events: ResponseEvent[]): {
  visibleEvents: ResponseEvent[];
  needsFallback: boolean;
} {
  let lastTextIdx = -1;
  for (let i = events.length - 1; i >= 0; i--) {
    if (isMeaningfulText(events[i])) {
      lastTextIdx = i;
      break;
    }
  }
  let visibleEvents: ResponseEvent[];
  if (lastTextIdx >= 0) {
    const preserved = events.slice(0, lastTextIdx).filter(
      e => e.type !== 'text' && !isStepMechanics(e)
    );
    visibleEvents = [...preserved, ...events.slice(lastTextIdx)];
  } else {
    visibleEvents = events;
  }
  const needsFallback = !visibleEvents.some(isMeaningfulText);
  return { visibleEvents, needsFallback };
}

/** Split events array into sections at section_break boundaries. */
export function splitEventSections(events: ResponseEvent[]): ResponseEvent[][] {
  const sections: ResponseEvent[][] = [];
  let current: ResponseEvent[] = [];
  for (const evt of events) {
    if (evt.type === 'section_break') {
      if (current.length > 0) sections.push(current);
      current = [];
    } else {
      current.push(evt);
    }
  }
  if (current.length > 0) sections.push(current);
  return sections;
}

/**
 * Merge consecutive text events into single text events.
 * Streaming deltas and tool-boundary splits can fragment a markdown document
 * (e.g., a code block split across two text events). Merging adjacent text
 * events ensures renderMarkdown() sees complete markdown structures.
 */
export function mergeAdjacentTextEvents(events: ResponseEvent[]): ResponseEvent[] {
  const merged: ResponseEvent[] = [];
  let textBuf = '';
  for (const evt of events) {
    if (evt.type === 'text') {
      textBuf += evt.md;
    } else {
      if (textBuf) {
        merged.push({ type: 'text', md: textBuf });
        textBuf = '';
      }
      merged.push(evt);
    }
  }
  if (textBuf) {
    merged.push({ type: 'text', md: textBuf });
  }
  return merged;
}
