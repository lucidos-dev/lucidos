import { describe, it, expect } from 'vitest';
import { parseNavigatedTurn } from '../scrollState';

// Pure decode of a navigated `.chat-exchange`'s collapse identity from its data
// attributes — no DOM, mirroring pick-turn-index.test.ts.

describe('parseNavigatedTurn', () => {
  it('decodes a response-body turn', () => {
    expect(parseNavigatedTurn('t-1', '4', 'response')).toEqual({
      threadId: 't-1', userSeq: 4, kind: 'response',
    });
  });

  it('decodes a response-less divider/change turn (initiator fallback)', () => {
    expect(parseNavigatedTurn('t-2', '0', 'initiator')).toEqual({
      threadId: 't-2', userSeq: 0, kind: 'initiator',
    });
  });

  it('returns null when the thread id is missing/empty', () => {
    expect(parseNavigatedTurn(null, '4', 'response')).toBeNull();
    expect(parseNavigatedTurn('', '4', 'response')).toBeNull();
  });

  it('returns null when the kind is absent or unknown (not collapsible)', () => {
    expect(parseNavigatedTurn('t-1', '4', null)).toBeNull();
    expect(parseNavigatedTurn('t-1', '4', 'both')).toBeNull();
    expect(parseNavigatedTurn('t-1', '4', '')).toBeNull();
  });

  it('returns null when the user seq is missing or non-integer', () => {
    expect(parseNavigatedTurn('t-1', null, 'response')).toBeNull();
    expect(parseNavigatedTurn('t-1', '', 'response')).toBeNull();
    expect(parseNavigatedTurn('t-1', 'x', 'response')).toBeNull();
    expect(parseNavigatedTurn('t-1', '4.5', 'response')).toBeNull();
  });
});
