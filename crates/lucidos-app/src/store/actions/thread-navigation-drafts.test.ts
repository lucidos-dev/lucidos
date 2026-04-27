import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue([]),
  fetchThreadMessages: vi.fn(),
  pinThread: vi.fn(),
  unpinThread: vi.fn(),
  dismissThread: vi.fn(),
}));

vi.mock('../../components/chat/promptFocus', () => ({
  focusPromptNow: vi.fn(),
  focusIfNeeded: vi.fn(),
  composeHandlers: vi.fn(),
}));

vi.mock('../../api/client', () => ({
  API_BASE: '',
  submitChat: vi.fn(),
  cancelChat: vi.fn(),
  cancelClaudeCode: vi.fn(),
  interruptClaudeCode: vi.fn(),
}));

import { focusedThreadId, focusedDraftId, drafts, newDraftId } from '../store';
import { focusThread, unfocusThread } from './threads';
import { createComposeDraft, focusDraft } from './drafts';
import {
  threadNavBack,
  threadNavForward,
  canGoBackThread,
  canGoForwardThread,
  _resetThreadNavForTesting,
} from './thread-navigation';

beforeEach(() => {
  localStorage.clear();
  drafts.value = new Map();
  focusedThreadId.value = null;
  focusedDraftId.value = newDraftId();
  _resetThreadNavForTesting();
});

describe('thread navigation — drafts in history', () => {
  it('createComposeDraft pushes a new entry to the nav stack', () => {
    focusThread('t1');
    expect(canGoBackThread.value).toBe(false);

    createComposeDraft();
    expect(canGoBackThread.value).toBe(true);

    threadNavBack();
    expect(focusedThreadId.value).toBe('t1');
  });

  it('focusDraft pushes a new entry to the nav stack', () => {
    focusThread('t1');

    focusDraft('draft-clicked');

    expect(focusedDraftId.value).toBe('draft-clicked');
    expect(canGoBackThread.value).toBe(true);

    threadNavBack();
    expect(focusedThreadId.value).toBe('t1');
  });

  it('back/forward between two compose drafts restores correct draft', () => {
    const d1 = createComposeDraft();
    const d2 = createComposeDraft();
    expect(focusedDraftId.value).toBe(d2);

    threadNavBack();
    expect(focusedThreadId.value).toBeNull();
    expect(focusedDraftId.value).toBe(d1);

    threadNavForward();
    expect(focusedThreadId.value).toBeNull();
    expect(focusedDraftId.value).toBe(d2);
  });

  it('thread → draft → thread navigates correctly back and forward', () => {
    focusThread('t1');
    focusDraft('d1');
    focusThread('t2');

    threadNavBack();
    expect(focusedThreadId.value).toBeNull();
    expect(focusedDraftId.value).toBe('d1');

    threadNavBack();
    expect(focusedThreadId.value).toBe('t1');

    threadNavForward();
    expect(focusedThreadId.value).toBeNull();
    expect(focusedDraftId.value).toBe('d1');

    threadNavForward();
    expect(focusedThreadId.value).toBe('t2');
  });

  it('unfocusThread (X button) pushes the current focused draft', () => {
    focusedDraftId.value = 'persistent-draft';
    focusThread('t1');

    unfocusThread();

    expect(focusedThreadId.value).toBeNull();
    expect(canGoBackThread.value).toBe(true);

    threadNavBack();
    expect(focusedThreadId.value).toBe('t1');

    threadNavForward();
    expect(focusedThreadId.value).toBeNull();
    expect(focusedDraftId.value).toBe('persistent-draft');
  });

  it('navigating back to a draft does not push another nav entry', () => {
    focusThread('t1');
    focusDraft('d1');
    focusThread('t2');

    threadNavBack();
    expect(focusedDraftId.value).toBe('d1');
    expect(canGoForwardThread.value).toBe(true);
  });

  it('focusing the same draft twice does not duplicate the entry', () => {
    focusDraft('d1');
    focusDraft('d1');

    expect(canGoBackThread.value).toBe(false);
  });
});
