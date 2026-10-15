import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../api/threads', () => ({
  renameThread: vi.fn(async () => {}),
  suggestTitle: vi.fn(async () => 'A suggested name'),
}));

vi.mock('../store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../store')>()),
  showPrompt: vi.fn(async () => null),
  showToast: vi.fn(),
}));

import { renameThread, suggestTitle } from '../../api/threads';
import { showPrompt, showToast, threadMap } from '../store';
import { makeThreadState } from './threads-test-helpers';
import { normalizeRename, promptRenameThread } from './threadRename';

const ID = 't1';

/** The arguments of the last toast raised. */
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

  it('renames the home thread like any other', async () => {
    threadMap.value = new Map([[ID, makeThreadState(ID, { meta: { home: true, title: 'Home' } })]]);
    vi.mocked(showPrompt).mockResolvedValueOnce('Kitchen table');
    await promptRenameThread(ID);
    expect(renameThread).toHaveBeenCalledWith(ID, 'Kitchen table');
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

/** The `suggest` option the rename dialog was opened with. */
async function suggestOption() {
  await promptRenameThread(ID);
  return vi.mocked(showPrompt).mock.calls[0][1]?.suggest;
}

describe('the rename dialog\'s Suggest link', () => {
  it('returns the suggestion for the field, without renaming', async () => {
    const suggest = await suggestOption();
    expect(await suggest!()).toBe('A suggested name');
    expect(renameThread).not.toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('fills nothing and says so when the suggestion is the current title', async () => {
    vi.mocked(suggestTitle).mockResolvedValueOnce('Old title');
    const suggest = await suggestOption();
    expect(await suggest!()).toBe(null);
    expect(lastToast()[2]?.title).toBe('No better name');
  });

  it('fills nothing and shows an error naming the thread when the suggestion fails', async () => {
    vi.mocked(suggestTitle).mockRejectedValueOnce(new Error('no provider'));
    const suggest = await suggestOption();
    expect(await suggest!()).toBe(null);
    const [message, type] = lastToast();
    expect(type).toBe('error');
    expect(message).toContain('Old title');
  });

  it('is not offered on the home thread, which only the user names', async () => {
    threadMap.value = new Map([[ID, makeThreadState(ID, { meta: { home: true } })]]);
    expect(await suggestOption()).toBeUndefined();
  });
});
