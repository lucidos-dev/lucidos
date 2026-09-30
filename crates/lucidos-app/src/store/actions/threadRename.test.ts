import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../api/threads', () => ({
  renameThread: vi.fn(async () => {}),
  suggestTitle: vi.fn(async () => 'A suggested name'),
}));

vi.mock('../store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../store')>()),
  showPrompt: vi.fn(async () => null),
  showToast: vi.fn(),
  removeToast: vi.fn(),
}));

import { renameThread, suggestTitle } from '../../api/threads';
import { removeToast, showPrompt, showToast, threadMap } from '../store';
import { makeThreadState } from './threads-test-helpers';
import { normalizeRename, promptRenameThread, suggestThreadName } from './threadRename';

const ID = 't1';

/** The options of the last toast raised, for reading its action. */
function lastToast() {
  const calls = vi.mocked(showToast).mock.calls;
  return calls[calls.length - 1];
}

beforeEach(() => {
  vi.clearAllMocks();
  threadMap.value = new Map([[ID, makeThreadState(ID, { meta: { title: 'Old title' } })]]);
});

describe('normalizeRename', () => {
  it('skips a blank or whitespace-only title', () => {
    expect(normalizeRename('', 'Old title')).toBe(null);
    expect(normalizeRename('   ', 'Old title')).toBe(null);
  });

  it('skips a title that only differs by surrounding whitespace', () => {
    expect(normalizeRename('  Old title  ', 'Old title')).toBe(null);
  });

  it('returns the trimmed new title', () => {
    expect(normalizeRename('  New title ', 'Old title')).toBe('New title');
  });
});

describe('promptRenameThread', () => {
  it('opens the rename dialog prefilled with the current title', async () => {
    await promptRenameThread(ID);
    expect(showPrompt).toHaveBeenCalledWith(expect.any(String), expect.objectContaining({ defaultValue: 'Old title' }));
  });

  it('renames to the trimmed value the user confirms', async () => {
    vi.mocked(showPrompt).mockResolvedValueOnce('  New title ');
    await promptRenameThread(ID);
    expect(renameThread).toHaveBeenCalledWith(ID, 'New title');
  });

  it('sends nothing when the user cancels, clears the field or keeps the title', async () => {
    for (const answer of [null, '', '  ', 'Old title']) {
      vi.mocked(showPrompt).mockResolvedValueOnce(answer);
      await promptRenameThread(ID);
    }
    expect(renameThread).not.toHaveBeenCalled();
  });

  it('shows an error toast naming the thread when the rename fails', async () => {
    vi.mocked(showPrompt).mockResolvedValueOnce('New title');
    vi.mocked(renameThread).mockRejectedValueOnce(new Error('boom'));
    await promptRenameThread(ID);
    const [message, type] = lastToast();
    expect(type).toBe('error');
    expect(message).toContain('Old title');
  });

  it('does nothing for a thread that is not loaded', async () => {
    await promptRenameThread('missing');
    expect(showPrompt).not.toHaveBeenCalled();
  });

  it('does nothing for a draft, whose compose text is its title (F2 reaches here too)', async () => {
    threadMap.value = new Map([[ID, makeThreadState(ID, { meta: { state: 'composing' } })]]);
    await promptRenameThread(ID);
    expect(showPrompt).not.toHaveBeenCalled();
  });
});

describe('suggestThreadName', () => {
  it('offers the suggestion without renaming', async () => {
    await suggestThreadName(ID);
    expect(renameThread).not.toHaveBeenCalled();
    const [message, , opts] = lastToast();
    expect(message).toContain('A suggested name');
    expect(opts?.action?.label).toBe('Use it');
  });

  it('renames to exactly the suggestion when the user takes it, and clears the offer', async () => {
    await suggestThreadName(ID);
    lastToast()[2]!.action!.onClick();
    await vi.waitFor(() => expect(renameThread).toHaveBeenCalledWith(ID, 'A suggested name'));
    expect(removeToast).toHaveBeenCalledWith(lastToast()[2]!.key);
  });

  it('shows a spinning toast while it waits, updated in place by the result', async () => {
    await suggestThreadName(ID);
    const calls = vi.mocked(showToast).mock.calls;
    expect(calls[0][2]?.spinning).toBe(true);
    expect(calls[0][2]?.key).toBeTruthy();
    expect(lastToast()[2]?.key).toBe(calls[0][2]?.key);
  });

  it('says so when the suggestion is the current title, with nothing to take', async () => {
    vi.mocked(suggestTitle).mockResolvedValueOnce('Old title');
    await suggestThreadName(ID);
    expect(lastToast()[2]?.action).toBeUndefined();
    // A keyed toast waits to be answered unless it names a delay.
    expect(lastToast()[2]?.autoDismissMs).toBeGreaterThan(0);
    expect(renameThread).not.toHaveBeenCalled();
  });

  it('does nothing for a draft', async () => {
    threadMap.value = new Map([[ID, makeThreadState(ID, { meta: { state: 'composing' } })]]);
    await suggestThreadName(ID);
    expect(suggestTitle).not.toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('shows an error toast naming the thread when the suggestion fails', async () => {
    vi.mocked(suggestTitle).mockRejectedValueOnce(new Error('no provider'));
    await suggestThreadName(ID);
    const [message, type] = lastToast();
    expect(type).toBe('error');
    expect(message).toContain('Old title');
  });
});
