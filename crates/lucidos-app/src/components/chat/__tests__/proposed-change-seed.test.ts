import { describe, expect, it } from 'vitest';
import { buildProposedChangeInfo } from '../CreateThreadView';
import type { Exchange } from '../../../store/thread-events';

function exchangeOf(...events: Record<string, unknown>[]): Exchange {
  return { steps: events.map(event => ({ event })) } as unknown as Exchange;
}

const proposed = (description: string, files: string[] = ['a.rs']) =>
  ({ type: 'ChangeProposed', change_id: 'c-1', description, files });
const summarized = (summary: string, description: string) =>
  ({ type: 'ChangeSummarized', change_id: 'c-1', summary, description });

/** The lifecycle card resolves the change's LAST proposal, so that is the
 *  seed, and a summary counts only while it summarized that list. */
describe('the change card seed', () => {
  it('takes the latest proposal and the summary of its commit list', () => {
    const seed = buildProposedChangeInfo([
      exchangeOf(proposed('b\na'), summarized('Old', 'b\na')),
      exchangeOf(proposed('c\nb\na', ['a.rs', 'b.rs']), summarized('Current', 'c\nb\na')),
    ]).get('c-1');
    expect(seed).toEqual({ description: 'c\nb\na', fileCount: 2, summary: 'Current' });
  });

  it('drops a summary of an older commit list', () => {
    const seed = buildProposedChangeInfo([
      exchangeOf(proposed('b\na'), summarized('Old', 'b\na')),
      exchangeOf(proposed('c\nb\na')),
    ]).get('c-1');
    expect(seed?.summary).toBeUndefined();
  });
});
