/**
 * Pure rendering utility functions for Exchange / ResponseEvent display.
 * No side effects, no signals — safe to import from components and tests.
 */
import type { ResponseEvent } from './types';

/** True when `e` is a text event whose markdown is non-empty after trimming. */
export function isMeaningfulText(e: ResponseEvent): boolean {
  return e.type === 'text' && e.md.trim().length > 0;
}

/** True when an in-progress step is actually rendered on screen — steps are
 *  expanded, the response panel is NOT collapsed, AND the visible set holds a
 *  pending step (`success === null`, the one that carries the `.running-shimmer`
 *  "live" affordance).
 *
 *  Drives the "exactly one running-text shimmer at a time" rule: when a live
 *  step is on screen its own shimmer is the live signal, so the status label
 *  ("Working") stays plain; otherwise the label itself shimmers as the sole
 *  affordance. The `collapsed` guard is load-bearing — a collapsed panel hides
 *  the steps body entirely (only the header status shows), so a pending step in
 *  the data is NOT on screen; without this, the label shimmer was suppressed
 *  while the step shimmer was hidden, leaving a working turn with no shimmer. */
export function hasVisibleLiveStep(showSteps: boolean, collapsed: boolean, visibleEvents: ResponseEvent[]): boolean {
  return showSteps && !collapsed && visibleEvents.some(e => e.type === 'step' && e.success === null);
}

/** Determine which toggles (More/Less, Show/Hide steps) to show for an exchange. */
export function getEventToggleState(events: ResponseEvent[]): {
  showMoreToggle: boolean;
  showStepsToggle: boolean;
} {
  const hasSteps = events.some(e => e.type === 'step');
  const meaningfulTextCount = events.filter(isMeaningfulText).length;
  const showMoreToggle = hasSteps && meaningfulTextCount >= 2;
  const showStepsToggle = hasSteps;
  return { showMoreToggle, showStepsToggle };
}

/** Determine which events are visible when the exchange is collapsed.
 *  Keeps events from the last text block onwards, plus section_break and
 *  image events (lifecycle markers that must always be visible). */
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
      e => e.type === 'section_break' || e.type === 'image'
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
