import { describe, it, expect } from 'vitest';
import { searchResultDestinationPane } from './searchEverywhereActions';

describe('searchResultDestinationPane', () => {
  it('routes thread results to the conversation (thread) pane', () => {
    expect(searchResultDestinationPane('threads')).toBe('thread');
  });

  it('routes every other category to the content pane', () => {
    for (const category of ['apps', 'files', 'settings', 'triggers', 'changes']) {
      expect(searchResultDestinationPane(category)).toBe('content');
    }
  });
});
