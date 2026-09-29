import { describe, expect, it } from 'vitest';
import { changeCommitList, changeHeadline, changeNamingFromEvents } from './changeHeadline';

const NEWEST_FIRST = 'fix(engine): settle a withdraw\nfeat: add change summaries';

describe('changeHeadline', () => {
  it('is the summary when a model has written one', () => {
    expect(changeHeadline({ description: NEWEST_FIRST, summary: 'Adds change summaries' }))
      .toBe('Adds change summaries');
  });

  it('is the oldest commit, never the newest, while there is no summary', () => {
    expect(changeHeadline({ description: NEWEST_FIRST })).toBe('feat: add change summaries');
    expect(changeHeadline({ description: NEWEST_FIRST, summary: '  ' })).toBe('feat: add change summaries');
  });

  it('prefers the applied commit list, which is oldest first already', () => {
    expect(changeHeadline({ description: NEWEST_FIRST, commits: ['feat: landed first', 'fix: landed last'] }))
      .toBe('feat: landed first');
  });

  it('is the one subject of a single-commit change', () => {
    expect(changeHeadline({ description: 'feat: one thing\n' })).toBe('feat: one thing');
  });

  it('falls back to a word when there is nothing to name it by', () => {
    expect(changeHeadline({})).toBe('Change');
    expect(changeHeadline({ description: ' \n ' })).toBe('Change');
  });
});

describe('changeCommitList', () => {
  it('reads a pending change oldest first, blank lines dropped', () => {
    expect(changeCommitList({ description: 'c: third\n\nb: second\na: first' }))
      .toEqual(['a: first', 'b: second', 'c: third']);
  });

  it('keeps an applied change in the order it landed', () => {
    expect(changeCommitList({ description: NEWEST_FIRST, commits: ['a', 'b'] })).toEqual(['a', 'b']);
  });
});

describe('changeNamingFromEvents', () => {
  const proposed = (description: string) => ({ type: 'ChangeProposed', change_id: 'c1', description });
  const summarized = (summary: string, description: string) =>
    ({ type: 'ChangeSummarized', change_id: 'c1', summary, description });

  it('names a change by its latest proposal and that list\'s summary', () => {
    const naming = changeNamingFromEvents(
      [proposed('b\na'), summarized('Old', 'b\na'), proposed('c\nb\na'), summarized('Current', 'c\nb\na')],
      'c1',
    );
    expect(naming).toEqual({ description: 'c\nb\na', summary: 'Current' });
  });

  it('ignores a summary of an older commit list', () => {
    const naming = changeNamingFromEvents([proposed('b\na'), summarized('Old', 'b\na'), proposed('c\nb\na')], 'c1');
    expect(naming && changeHeadline(naming)).toBe('a');
  });

  it('has nothing to say without a proposal, or about another change', () => {
    expect(changeNamingFromEvents([summarized('S', 'a')], 'c1')).toBeUndefined();
    expect(changeNamingFromEvents([proposed('a')], 'other')).toBeUndefined();
  });
});
