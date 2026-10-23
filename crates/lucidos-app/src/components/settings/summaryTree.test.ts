import { describe, it, expect } from 'vitest';
import { ENTRY_KINDS } from '@lucidos/engine-constants';
import { dateRangeLabel, entriesLabel, entryKindOf, isSourceText, lineSpan, pendingNote } from './summaryTree';
import { formatShortDateWithYear, formatShortTime } from '../../utils/formatTime';
import { rangeLabel } from '../../utils/rangeLabel';

describe('summary tree line helpers', () => {
  it('reads the span off a workspace or thread node id', () => {
    expect(lineSpan('w/8+8')).toBe(8);
    expect(lineSpan('3f2c1d4e-0000-4000-8000-000000000000/12+1')).toBe(1);
    expect(lineSpan('w/pending')).toBeNull();
  });

  it('counts entries in words', () => {
    expect(entriesLabel(1)).toBe('1 entry');
    expect(entriesLabel(1024)).toBe(`${(1024).toLocaleString()} entries`);
  });

  it('tells a leaf source from finer lines', () => {
    expect(isSourceText('w/4+1', [{ id: 'w/4+1', text: 'artifact: notes.md' }])).toBe(true);
    expect(isSourceText('w/0+2', [{ id: 'w/0+1', text: 'a' }, { id: 'w/1+1', text: 'b' }])).toBe(false);
    expect(isSourceText('w/0+2', [{ id: 'w/0+1', text: 'a' }])).toBe(false);
  });

  it('shows one moment once and a span of days as a range', () => {
    const at = '2026-09-19T12:00:00Z';
    const [from, to] = [new Date('2026-09-02T12:00:00Z'), new Date(at)];
    expect(dateRangeLabel(at, at)).toBe(`${formatShortDateWithYear(to)}, ${formatShortTime(to)}`);
    expect(dateRangeLabel(from.toISOString(), at))
      .toBe(rangeLabel(formatShortDateWithYear(from), formatShortDateWithYear(to)));
  });

  it('shows hours of one day as a range of times', () => {
    const [from, to] = [new Date('2026-09-19T12:00:00Z'), new Date('2026-09-19T12:45:00Z')];
    expect(dateRangeLabel(from.toISOString(), to.toISOString()))
      .toBe(`${formatShortDateWithYear(from)}, ${rangeLabel(formatShortTime(from), formatShortTime(to))}`);
  });

  it('never draws an en or em dash between the ends', () => {
    for (const to of ['2026-09-19T12:45:00Z', '2026-10-09T12:00:00Z']) {
      expect(dateRangeLabel('2026-09-19T12:00:00Z', to)).not.toMatch(/[\u2013\u2014]/);
    }
  });

  it('words the pending line for the browser, which has no search', () => {
    expect(pendingNote('3 newer entries are not summarised yet. search finds their words'))
      .toBe('3 newer entries are not summarised yet');
    expect(pendingNote('1 newer entries are not summarised yet. search finds their words'))
      .toBe('1 newer entry is not summarised yet');
    expect(pendingNote('something else')).toBe('something else');
  });

  it('reads the entry kind a line starts with, as the engine writes it', () => {
    for (const kind of ENTRY_KINDS) {
      expect(entryKindOf(`${kind}: body`)).toEqual({ kind, text: 'body' });
    }
  });

  it('keeps an artifact path written without a space, and the body under it', () => {
    expect(entryKindOf('artifact:notes/plan.md')).toEqual({ kind: 'artifact', text: 'notes/plan.md' });
    expect(entryKindOf('artifact: notes/plan.md\nfirst line')).toEqual({ kind: 'artifact', text: 'notes/plan.md\nfirst line' });
  });

  it('leaves a line that starts with no entry kind as it is', () => {
    for (const text of [
      'turn [Tour] prompt (human): rehearse. response: done',
      'Result: capitalised is not a kind',
      'responses: not a kind either',
      'a summary line',
    ]) {
      expect(entryKindOf(text)).toEqual({ kind: null, text });
    }
  });
});
