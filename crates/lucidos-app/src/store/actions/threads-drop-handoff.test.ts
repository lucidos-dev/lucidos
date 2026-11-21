/**
 * Deleting the open thread hands the focus to the next row the way a tap opens
 * it, so a Blocked row lands on its card. A plain focus opened it on its saved
 * reading position. This runs for a delete made on another device too.
 */
import { describe, it, expect, vi } from 'vitest';

const openListedThread = vi.hoisted(() => vi.fn());
vi.mock('./event-navigation', () => ({ openListedThread }));
vi.mock('./threads', () => ({
  unfocusThread: vi.fn(),
  visibleCandidatesAround: () => ['gone', 'next'],
}));

const { dropDeletedThreads } = await import('./threads-drop');
const { focusedThreadId, threadMap } = await import('../store');

describe('the delete hand-off', () => {
  it('opens the next row as a listed row, without revealing the pane', () => {
    threadMap.value = new Map();
    focusedThreadId.value = 'gone';

    dropDeletedThreads(['gone']);

    expect(openListedThread).toHaveBeenCalledWith('next', { revealPane: false });
  });
});
