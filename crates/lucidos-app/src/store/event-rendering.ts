import type { ResponseEvent } from './types';

export function hasVisibleText(text: string | undefined): boolean {
  return !!text && text.trim().length > 0;
}

export function isMeaningfulText(e: ResponseEvent): boolean {
  return e.type === 'text' && hasVisibleText(e.md);
}

export function isStepMechanics(event: ResponseEvent): event is Extract<ResponseEvent, { type: 'step' }> {
  return event.type === 'step';
}

export function drawsResponseRow(event: ResponseEvent, showSteps: boolean): boolean {
  if (isStepMechanics(event)) return showSteps;
  switch (event.type) {
    case 'text': return isMeaningfulText(event);
    case 'image':
    case 'checkpoint':
    case 'widget':
    case 'event_wait':
    case 'held_message':
    case 'cache_keep_alive':
    case 'form_request':
    case 'spoken_reply':
    case 'restart':
    case 'fresh_session':
    case 'handoff_session':
    case 'empty':
      return true;
    case 'question':
    case 'permission':
    case 'section_break':
      return false;
  }
}

export function hidesEarlierProse(events: ResponseEvent[]): boolean {
  const hasSteps = events.some(isStepMechanics);
  const meaningfulTextCount = events.filter(isMeaningfulText).length;
  return hasSteps && meaningfulTextCount >= 2;
}

export function headClampApplies(events: ResponseEvent[], showDetails: boolean): boolean {
  return showDetails || !hidesEarlierProse(events);
}

export interface TurnView {
  showSteps: boolean;
  showDetails: boolean;
  folded: boolean;
}

export function rowsDrawnByClamp(events: ResponseEvent[], view: TurnView): boolean[] {
  if (view.folded || !headClampApplies(events, view.showDetails)) return events.map(() => false);
  return events.map(e => drawsResponseRow(e, view.showSteps));
}

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

export type BodyRow =
  | { kind: 'steps'; key: number; steps: { event: StepEvent; index: number }[]; open: boolean; elided: boolean }
  | { kind: 'text'; key: number | string; event: TextEvent; open: boolean }
  | { kind: 'marker'; key: number; event: ResponseEvent };

export interface BodySection {
  key: number;
  rows: BodyRow[];
}

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
        if (!isMeaningfulText(event)) break;
        run = null;
        if (index >= from) section.rows.push({ kind: 'text', key: index, event, open: shown(event) });
        break;
      case 'question':
      case 'permission':
        break;
      case 'image':
      case 'checkpoint':
      case 'widget':
      case 'event_wait':
      case 'held_message':
      case 'cache_keep_alive':
      case 'form_request':
      case 'spoken_reply':
      case 'restart':
      case 'fresh_session':
      case 'handoff_session':
      case 'empty':
        run = null;
        if (index >= from) section.rows.push({ kind: 'marker', key: index, event });
        break;
      default:
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

export function mergeAdjacentTextEvents(events: ResponseEvent[]): ResponseEvent[] {
  const merged: ResponseEvent[] = [];
  let textBuf = '';
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
