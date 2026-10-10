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
 *  An ALLOW-list, mirroring `responseBody` below arm for arm, because the body
 *  draws a row for some event kinds and nothing for the rest. A deny-list
 *  ("anything that isn't a blank text") reads as the safe direction and is
 *  not: `question` and `permission`
 *  land in an exchange's events and draw nothing HERE, since they render as
 *  initiator-panel dividers with their own fold (`collapsedInitiators`), and a
 *  `section_break` splits `responseBody` into sections.
 *
 *  Of the kinds that do draw, two are conditional: a `text` needs visible text
 *  (one is pushed for every `CodingAgentTextStreamed`, and a torn-down
 *  subprocess signs off with a bare `"\n\n"`), and step mechanics need the
 *  steps control to be on.
 *
 *  Deliberately NOT `hasRenderableResponseContent`, which is the same shape of
 *  question asked for a different purpose and reaches a different answer about
 *  a step. That one decides whether a turn is worth a PANEL, so it counts a
 *  hidden step: the header carries the control that reveals it, and a panel you
 *  can open is not a dead end. This one decides whether there is anything to
 *  FOLD. A fold hides the body and lights the collapse control. On a turn
 *  drawing nothing, it hides nothing and lights anyway, which is a false signal. The two
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
    case 'widget':
    case 'event_wait':
    case 'held_message':
    case 'form_request':
    case 'spoken_reply':
    case 'empty':
      return true;
    // Drawn somewhere else, or not at all. A question and a permission are
    // initiator-panel dividers with their own fold; a `section_break` splits
    // `responseBody` into sections and draws no row.
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

/** Does the render window's head clamp decide what this turn's body draws?
 *
 *  Not on the collapsed-prose path: turning details off keeps what follows the
 *  last prose chunk, whatever the clamp says. `ChatExchange` picks its path by
 *  this, and the window reads it through `rowsDrawnByClamp`. */
export function headClampApplies(events: ResponseEvent[], showDetails: boolean): boolean {
  return showDetails || !hidesEarlierProse(events);
}

/** The reader's view of one turn, as far as its body is concerned. */
export interface TurnView {
  showSteps: boolean;
  showDetails: boolean;
  /** The reader folded this turn down to its header. */
  folded: boolean;
}

/** Which of a turn's rows the CLAMPED slice draws, one flag per row.
 *
 *  What the render window budgets by (`threadWindow.ts`). A row that renders
 *  `null` costs nothing and adds no height, so counting it strands the reader:
 *  a window can fill its whole budget with rows the reader cannot see.
 *
 *  A folded turn mounts no body. A turn on the collapsed-prose path ignores the
 *  clamp. Either way the clamp draws nothing, so every flag is false. */
export function rowsDrawnByClamp(events: ResponseEvent[], view: TurnView): boolean[] {
  if (view.folded || !headClampApplies(events, view.showDetails)) return events.map(() => false);
  return events.map(e => drawsResponseRow(e, view.showSteps));
}

/** Determine which events are visible when the exchange is collapsed.
 *  Keeps events from the last text block onwards, plus every marker before it
 *  (see `isStepMechanics`): the collapse drops the mechanics and the superseded
 *  prose, never the record that something happened. */
export function getCollapsedVisibleEvents(events: ResponseEvent[]): ResponseEvent[] {
  let lastTextIdx = -1;
  for (let i = events.length - 1; i >= 0; i--) {
    if (isMeaningfulText(events[i])) {
      lastTextIdx = i;
      break;
    }
  }
  if (lastTextIdx < 0) return events;
  const preserved = events.slice(0, lastTextIdx).filter(
    e => e.type !== 'text' && !isStepMechanics(e)
  );
  return [...preserved, ...events.slice(lastTextIdx)];
}

type StepEvent = Extract<ResponseEvent, { type: 'step' }>;
type TextEvent = Extract<ResponseEvent, { type: 'text' }>;

/** One row of a response body. `key` is the row's index in the turn's whole
 *  event list, so neither toggle, the head clamp, nor a new chunk moves it.
 *  A moved key remounts the row, and a remounted row cannot roll.
 *
 *  `open` is whether the row is drawn. A closed row still renders, as a
 *  `<Disclosure>` that rolls it away and then draws nothing. */
export type BodyRow =
  /** Consecutive steps with no drawn row between them: one Disclosure.
   *  `elided` is a closed run between two drawn chunks. It draws the hairline
   *  that says steps were hidden there, as a row of its own that rolls. */
  | { kind: 'steps'; key: number; steps: { event: StepEvent; index: number }[]; open: boolean; elided: boolean }
  /** A chunk's tail split off at a side question is keyed `<key>@<offset>`,
   *  which holds while the chunk keeps growing. */
  | { kind: 'text'; key: number | string; event: TextEvent; open: boolean }
  /** A transcript marker. Neither toggle ever hides one. */
  | { kind: 'marker'; key: number; event: ResponseEvent };

export interface BodySection {
  /** Index of the section's first event, so it is as stable as its rows. */
  key: number;
  rows: BodyRow[];
}

/** A turn's body as `ChatExchange` draws it: sections of rows, split at
 *  `section_break`, each row told whether the reader's settings draw it.
 *
 *  With the head clamp in force it draws every row from `rowsHidden` on.
 *  Without it (the full response off) it draws `getCollapsedVisibleEvents`.
 *  Steps also need the steps control on. */
export function responseBody(
  events: ResponseEvent[],
  view: { showSteps: boolean; showDetails: boolean; rowsHidden: number },
): BodySection[] {
  const clamped = headClampApplies(events, view.showDetails);
  const from = clamped ? view.rowsHidden : 0;
  const kept = clamped ? null : new Set(getCollapsedVisibleEvents(events));
  const shown = (e: ResponseEvent) => kept === null || kept.has(e);

  const sections: BodySection[] = [];
  let section: BodySection = { key: 0, rows: [] };
  let run: Extract<BodyRow, { kind: 'steps' }> | null = null;
  for (const [index, event] of events.entries()) {
    switch (event.type) {
      case 'step':
        // A run is keyed by its first step, even when the clamp cuts that step.
        if (!run) {
          run = { kind: 'steps', key: index, steps: [], open: view.showSteps && shown(event), elided: false };
          section.rows.push(run);
        }
        if (index >= from) run.steps.push({ event, index });
        break;
      case 'section_break':
        sections.push(section);
        section = { key: index + 1, rows: [] };
        run = null;
        break;
      case 'text':
        // A blank chunk draws nothing, so it does not split a run.
        if (!isMeaningfulText(event)) break;
        run = null;
        if (index >= from) section.rows.push({ kind: 'text', key: index, event, open: shown(event) });
        break;
      // Drawn as initiator-panel dividers, so they split nothing here.
      case 'question':
      case 'permission':
        break;
      case 'image':
      case 'checkpoint':
      case 'widget':
      case 'event_wait':
      case 'held_message':
      case 'form_request':
      case 'spoken_reply':
      case 'empty':
        run = null;
        if (index >= from) section.rows.push({ kind: 'marker', key: index, event });
        break;
      default:
        // A new event kind fails to compile here until someone places it.
        event satisfies never;
    }
  }
  sections.push(section);
  return sections
    .map(s => ({ ...s, rows: markElisions(s.rows.filter(r => r.kind !== 'steps' || r.steps.length > 0)) }))
    .filter(s => s.rows.length > 0);
}

const drawnChunk = (row: BodyRow | undefined) => row?.kind === 'text' && row.open;

function markElisions(rows: BodyRow[]): BodyRow[] {
  return rows.map((row, i) => (
    row.kind === 'steps'
      ? { ...row, elided: !row.open && drawnChunk(rows[i - 1]) && drawnChunk(rows[i + 1]) }
      : row
  ));
}

/** Where the FIRST drawn live step sits in the turn's events, or -1 for none.
 *
 *  Live means `outcome === 'pending'`, the row carrying the `.running-shimmer`.
 *  The question is what SHIMMERS, not what is terminal. `'unfinished'` is
 *  terminal and does not. `'blocked'` is not terminal and still does not: a
 *  call held on a permission card is waiting for the reader.
 *
 *  Half of the "exactly one running-text shimmer at a time" rule. Drawn is not
 *  seen, so `ChatExchange` narrows this with `useOnScreenInTranscript` over
 *  the row's own element before it drops the "Working" label's shimmer. It
 *  also asks it only of an unfolded turn: a folded one draws no step at all.
 *
 *  First rather than last, because of where the label sits. Parallel calls
 *  each push a pending row, and the "Working" label is above all of them. A
 *  row below the first is below the fold whenever the first is. */
export function liveStepInBody(sections: BodySection[]): number {
  for (const { rows } of sections) {
    for (const row of rows) {
      if (row.kind !== 'steps' || !row.open) continue;
      const live = row.steps.find(s => s.event.outcome === 'pending');
      if (live) return live.index;
    }
  }
  return -1;
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
  // A merged chunk sits where its first piece did on the thread's clock. It
  // remembers where each later piece begins, so a side question can split it.
  let pieces: { at: number; seq?: number }[] = [];
  const flushText = () => {
    if (textBuf) {
      merged.push({ type: 'text', md: textBuf, seq: pieces[0]?.seq, ...(pieces.length > 1 ? { pieces } : {}) });
    }
    textBuf = '';
    pieces = [];
  };
  for (const evt of events) {
    if (evt.type === 'text') {
      if (evt.md) pieces.push({ at: textBuf.length, seq: evt.seq });
      textBuf += evt.md;
    } else {
      flushText();
      merged.push(evt);
    }
  }
  flushText();
  return merged;
}
