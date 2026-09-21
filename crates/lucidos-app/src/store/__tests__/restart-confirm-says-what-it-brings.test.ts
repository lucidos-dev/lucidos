// @vitest-environment jsdom
/**
 * The confirm every restart surface opens, in its new-version shape.
 *
 * Driven through the real `showConfirm`, so what is asserted is the dialog
 * state the user would be looking at. The copy itself is pinned next door in
 * `restartConfirmCopy.test.ts`; this file is about the wiring: which shape is
 * raised, and what each answer does.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/platform')>()),
  // The desktop shell's extra "Restart App" action is not what this covers.
  isTauri: () => false,
}));
vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  restartEngine: vi.fn(() => Promise.resolve()),
}));

import { confirmAndRestartEngine } from '../actions/chat-changes';
import { restartEngine } from '../../api/client';
import type { PendingCommits } from '../../api/client';
import {
  confirmState,
  engineVersionReady,
  enginePackaged,
  enginePendingCommits,
  engineRestarting,
  restartGroups,
  restartRequired,
  showToast,
  toasts,
  NEW_VERSION_TOAST_KEY,
} from '../store';
import { markEngineVersionDismissed } from '../../hooks/sw-update';

vi.mock('../../hooks/sw-update', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../hooks/sw-update')>()),
  markEngineVersionDismissed: vi.fn(),
}));

const COMMITS: PendingCommits = {
  total: 2,
  groups: [{ kind: 'new', total: 2, descriptions: ['a thing', 'another thing'] }],
};

/** Answer the confirm currently on screen. */
function answer(value: boolean): void {
  confirmState.peek().resolve?.(value);
}

/** The standing offer the toast makes, so a decline can be shown to leave it. */
function raiseVersionToast(): void {
  showToast('New version available.', 'info', {
    key: NEW_VERSION_TOAST_KEY,
    action: { label: 'Switch to new version', onClick: () => {} },
  });
}

beforeEach(() => {
  localStorage.clear();
  toasts.value = [];
  confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
  engineVersionReady.value = true;
  enginePackaged.value = false;
  engineRestarting.value = false;
  restartRequired.value = false;
  restartGroups.value = [];
  enginePendingCommits.value = COMMITS;
  vi.mocked(restartEngine).mockClear();
  vi.mocked(markEngineVersionDismissed).mockClear();
});

describe('a new version is ready', () => {
  it('asks in the new-version shape, listing what the switch brings', () => {
    void confirmAndRestartEngine();
    const state = confirmState.value;
    expect(state.visible).toBe(true);
    expect(state.title).toBe('New version available');
    expect(state.okLabel).toBe('Switch to new version');
    expect(state.cancelLabel).toBe('Later');
    expect(state.details?.groups).toEqual([
      { header: 'New', items: ['a thing', 'another thing'] },
    ]);
    answer(false);
  });

  it('switches on the OK, which is the only thing that does', async () => {
    const asked = confirmAndRestartEngine();
    answer(true);
    await asked;
    expect(restartEngine).toHaveBeenCalledOnce();
    expect(engineRestarting.value).toBe(true);
  });

  it('declining leaves the offer exactly as it was', async () => {
    raiseVersionToast();
    const asked = confirmAndRestartEngine();
    answer(false);
    await asked;
    expect(restartEngine).not.toHaveBeenCalled();
    expect(engineRestarting.value).toBe(false);
    // Declining is not deferring: the toast stays up and this build is not
    // remembered as dismissed, so the offer is still standing.
    expect(toasts.value.some((t) => t.key === NEW_VERSION_TOAST_KEY)).toBe(true);
    expect(markEngineVersionDismissed).not.toHaveBeenCalled();
  });
});

describe('nothing newer', () => {
  it('asks the short question it always asked', () => {
    engineVersionReady.value = false;
    restartGroups.value = [{ threadId: 't1', threadTitle: 'A thread', commits: ['fix: a thing'] }];
    void confirmAndRestartEngine();
    const state = confirmState.value;
    expect(state.title).toBeUndefined();
    expect(state.message).toBe('Restart engine?');
    expect(state.okLabel).toBe('Restart');
    expect(state.details?.intro).toBe('These changes will be applied:');
    answer(false);
  });
});
