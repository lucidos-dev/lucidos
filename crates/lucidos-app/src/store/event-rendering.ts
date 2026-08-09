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
 *  `.running-shimmer` "live" affordance). Only `'pending'` counts: every other
 *  outcome, `'unfinished'` included, is terminal.
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
 *  here rather than each carrying its own list. "Show steps" hides mechanics;
 *  the collapsed (Less) view drops mechanics and earlier prose to leave the
 *  final answer. Hiding a marker is a different thing entirely: the fact goes
 *  with it, and none of these is reachable anywhere else once it is gone.
 *
 *  The event-wait row is the case that proved it. It was classed as a step, so
 *  it was hidden by a default-off toggle AND dropped by a collapse that kept
 *  only two of the markers, and a parked thread showed no evidence anywhere in
 *  the transcript that it had parked. */
export function isStepMechanics(event: ResponseEvent): boolean {
  return event.type === 'step';
}

/** Determine which toggles (More/Less, Show/Hide steps) to show for an exchange. */
export function getEventToggleState(events: ResponseEvent[]): {
  showMoreToggle: boolean;
  showStepsToggle: boolean;
} {
  const hasSteps = events.some(isStepMechanics);
  const meaningfulTextCount = events.filter(isMeaningfulText).length;
  const showMoreToggle = hasSteps && meaningfulTextCount >= 2;
  const showStepsToggle = hasSteps;
  return { showMoreToggle, showStepsToggle };
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
