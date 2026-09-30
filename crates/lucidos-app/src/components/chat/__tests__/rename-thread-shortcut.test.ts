/**
 * The Rename thread shortcut (F2) opens the rename dialog for the focused
 * thread, and does nothing with no thread focused. The title is display-only,
 * so the dialog is the one place a rename happens.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../../store/actions/threadRename', () => ({
  promptRenameThread: vi.fn(async () => {}),
}));

import { promptRenameThread } from '../../../store/actions/threadRename';
import { focusedThreadId } from '../../../store/store';
import { dispatchForwardedChord } from '../../../hooks/useKeyboardShortcuts';

const F2 = { metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: 'F2' };

beforeEach(() => {
  vi.mocked(promptRenameThread).mockClear();
});

describe('Rename thread shortcut', () => {
  it('opens the rename dialog for the focused thread', () => {
    focusedThreadId.value = 't1';
    dispatchForwardedChord(F2);
    expect(promptRenameThread).toHaveBeenCalledWith('t1');
  });

  it('does nothing with no thread focused', () => {
    focusedThreadId.value = null;
    dispatchForwardedChord(F2);
    expect(promptRenameThread).not.toHaveBeenCalled();
  });
});
