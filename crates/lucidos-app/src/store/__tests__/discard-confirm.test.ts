import { describe, it, expect, beforeEach, vi } from 'vitest';

// Discard deletes a change's branch and worktree, so the Changes panel's
// Discard and Discard All ask first, as the thread's own Discard does.
const showConfirm = vi.fn();
vi.mock('../store', async () => {
  const actual = await vi.importActual<typeof import('../store')>('../store');
  return { ...actual, showConfirm: (...args: unknown[]) => showConfirm(...args) };
});

const discardChange = vi.fn();
const discardAll = vi.fn();
vi.mock('../../api/client', async () => {
  const actual = await vi.importActual<typeof import('../../api/client')>('../../api/client');
  return {
    ...actual,
    discardChange: (...args: unknown[]) => discardChange(...args),
    discardAllChanges: (...args: unknown[]) => discardAll(...args),
  };
});

const { discardSingleChange, discardAllChanges } = await import('../actions/chat-changes');

beforeEach(() => {
  showConfirm.mockReset();
  discardChange.mockReset().mockResolvedValue(undefined);
  discardAll.mockReset().mockResolvedValue({ discarded: 1, failed: 0, errors: [] });
});

describe('discarding from the Changes panel asks first', () => {
  it('discards nothing when the user declines', async () => {
    showConfirm.mockResolvedValue(false);
    await discardSingleChange('c1');
    await discardAllChanges();
    expect(discardChange).not.toHaveBeenCalled();
    expect(discardAll).not.toHaveBeenCalled();
  });

  it('discards once the user confirms', async () => {
    showConfirm.mockResolvedValue(true);
    await discardSingleChange('c1');
    await discardAllChanges();
    expect(discardChange).toHaveBeenCalledWith('c1');
    expect(discardAll).toHaveBeenCalledTimes(1);
    expect(showConfirm).toHaveBeenCalledWith(
      expect.stringContaining('cannot be undone'),
      'Discard',
      { variant: 'danger' },
    );
  });
});
