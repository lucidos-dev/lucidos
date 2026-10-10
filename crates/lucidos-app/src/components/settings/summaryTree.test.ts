import { describe, it, expect } from 'vitest';
import { dateRangeLabel, entriesLabel, isSourceText, lineSpan, pendingNote } from './summaryTree';

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
    expect(dateRangeLabel(at, at)).not.toContain('–');
    expect(dateRangeLabel('2026-09-02T12:00:00Z', '2026-09-19T12:00:00Z')).toContain('–');
  });

  it('words the pending line for the browser, which has no search', () => {
    expect(pendingNote('3 newer entries are not summarised yet. search finds their words'))
      .toBe('3 newer entries are not summarised yet');
    expect(pendingNote('1 newer entries are not summarised yet. search finds their words'))
      .toBe('1 newer entry is not summarised yet');
    expect(pendingNote('something else')).toBe('something else');
  });
});
