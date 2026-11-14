import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ReleaseNotice, ReleaseNoticeView } from '../../api/client';

const resolveReleaseNotice = vi.fn();
const sendSeededPrompt = vi.fn();
const openSettingsSubview = vi.fn();

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  releaseNotices: vi.fn(),
  resolveReleaseNotice: (...a: unknown[]) => resolveReleaseNotice(...a),
}));

vi.mock('./compose', () => ({
  sendSeededPrompt: (...a: unknown[]) => sendSeededPrompt(...a),
}));

vi.mock('./menu', () => ({
  openSettingsSubview: (...a: unknown[]) => openSettingsSubview(...a),
}));

const {
  acknowledgeReleaseNotice,
  dismissReleaseNoticeModal,
  openReleaseNoticesPanel,
  takeReleaseNoticeAction,
} = await import('./releaseNotices');
// The reads live a layer up, away from anything that can send a prompt. Both
// halves are exercised together here: they are one behaviour, and the split is
// about the import graph rather than about what a notice does.
const {
  owedReleaseNotice,
  owedReleaseNoticeCount,
  owedReleaseNoticeTitles,
  releaseNoticeActionLabel,
  releaseNoticeDismissed,
  releaseNoticeModalOpen,
} = await import('../releaseNotices');
const { releaseNoticeView } = await import('../store');

function notice(id: string, resolved: boolean, action = false, action_deferred = false): ReleaseNotice {
  return {
    id,
    since: '2.0.0',
    title: `Notice ${id}`,
    body: 'Do the thing.',
    resolved,
    action_deferred,
    ...(action ? { action_label: 'Do it', action_prompt: 'Do the thing for me.' } : {}),
  };
}

function loaded(notices: ReleaseNotice[], next_id: string | null): void {
  releaseNoticeView.value = { status: 'loaded', data: { notices, next_id } };
}

/** What the engine answers a resolve with: the same list, settled. */
function settled(notices: ReleaseNotice[], next_id: string | null): ReleaseNoticeView {
  return { notices, next_id };
}

describe('the notice the modal owes', () => {
  beforeEach(() => {
    releaseNoticeDismissed.value = false;
    releaseNoticeView.value = { status: 'not-loaded' };
    vi.clearAllMocks();
  });

  it('is nothing until the list loads', () => {
    expect(owedReleaseNotice()).toBe(null);
    expect(releaseNoticeModalOpen()).toBe(false);
  });

  it('is the one the engine named, never the first unresolved row', () => {
    // The engine decides, because it knows the cursor and the running release.
    // Re-deriving here is how the two would drift.
    loaded([notice('a', true), notice('b', false), notice('c', false)], 'b');
    expect(owedReleaseNotice()?.id).toBe('b');
  });

  it('lists and counts what is still owed, so the modal can name it', () => {
    loaded([notice('a', true), notice('b', false), notice('c', false)], 'b');
    expect(owedReleaseNoticeTitles()).toEqual(['Notice b', 'Notice c']);
    expect(owedReleaseNoticeCount()).toBe(2);
  });

  it('is nothing once the workspace has answered everything', () => {
    loaded([notice('a', true)], null);
    expect(owedReleaseNotice()).toBe(null);
    expect(releaseNoticeModalOpen()).toBe(false);
  });
});

describe('answering', () => {
  beforeEach(() => {
    releaseNoticeDismissed.value = false;
    releaseNoticeView.value = { status: 'not-loaded' };
    vi.clearAllMocks();
  });

  it('makes the next notice the owed one', async () => {
    loaded([notice('a', false), notice('b', false)], 'a');
    resolveReleaseNotice.mockResolvedValue(settled([notice('a', true), notice('b', false)], 'b'));

    await acknowledgeReleaseNotice(notice('a', false));

    expect(resolveReleaseNotice).toHaveBeenCalledWith('a');
    expect(owedReleaseNotice()?.id).toBe('b');
    expect(releaseNoticeModalOpen()).toBe(true);
  });

  it('leaves the notice owed when the engine refused the answer', async () => {
    // Treating a failure as answered would spend the one time the reader is
    // told, and nothing would ever raise it again.
    loaded([notice('a', false)], 'a');
    resolveReleaseNotice.mockRejectedValue(new Error('offline'));

    await acknowledgeReleaseNotice(notice('a', false));

    expect(owedReleaseNotice()?.id).toBe('a');
  });
});

describe('acting on a notice', () => {
  beforeEach(() => {
    releaseNoticeDismissed.value = false;
    releaseNoticeView.value = { status: 'not-loaded' };
    vi.clearAllMocks();
  });

  it('sends the notice sentence and answers it', async () => {
    loaded([notice('a', false, true), notice('b', false)], 'a');
    sendSeededPrompt.mockResolvedValue(true);
    resolveReleaseNotice.mockResolvedValue(
      settled([notice('a', true, true), notice('b', false)], 'b'),
    );

    await takeReleaseNoticeAction(notice('a', false, true));

    expect(sendSeededPrompt).toHaveBeenCalledWith('Do the thing for me.', expect.any(String));
    expect(resolveReleaseNotice).toHaveBeenCalledWith('a');
    // The action runs from the panel, which stays put. It leaves the modal's
    // dismissal alone, so 'b' still announces itself on the next open.
    expect(releaseNoticeDismissed.value).toBe(false);
    expect(owedReleaseNotice()?.id).toBe('b');
  });

  it('answers nothing when the send did not happen', async () => {
    // A failed send starts nothing, so the notice is still owed.
    loaded([notice('a', false, true)], 'a');
    sendSeededPrompt.mockResolvedValue(false);

    await takeReleaseNoticeAction(notice('a', false, true));

    expect(resolveReleaseNotice).not.toHaveBeenCalled();
    expect(owedReleaseNotice()?.id).toBe('a');
  });

  /** **Got it says "I have read this", never "I have done it".** So the action
   *  outlives the answer, and the Release Notices page keeps its button live on
   *  an answered row. Greyed out, it read as already carried out. */
  it('still runs an already answered notice, without answering it twice', async () => {
    loaded([notice('a', true, true)], null);
    sendSeededPrompt.mockResolvedValue(true);

    await takeReleaseNoticeAction(notice('a', true, true));

    expect(sendSeededPrompt).toHaveBeenCalledWith('Do the thing for me.', expect.any(String));
    // The answer is already recorded. Recording it again would append an event
    // saying exactly what the first one said.
    expect(resolveReleaseNotice).not.toHaveBeenCalled();
  });
});

describe('dismissing without answering', () => {
  beforeEach(() => {
    releaseNoticeDismissed.value = false;
    releaseNoticeView.value = { status: 'not-loaded' };
    vi.clearAllMocks();
  });

  // Later, Escape, the X and an outside click all land here.
  it('closes the modal and resolves nothing, so the notice returns', () => {
    loaded([notice('a', false)], 'a');

    dismissReleaseNoticeModal();

    expect(resolveReleaseNotice).not.toHaveBeenCalled();
    expect(releaseNoticeModalOpen()).toBe(false);
    // Still owed. A reload asks again, which is what "not now" means.
    expect(owedReleaseNotice()?.id).toBe('a');
  });
});

describe('opening the panel', () => {
  beforeEach(() => {
    releaseNoticeDismissed.value = false;
    releaseNoticeView.value = { status: 'not-loaded' };
    vi.clearAllMocks();
  });

  it('closes the modal, lands the Release Notices panel, and answers nothing', () => {
    loaded([notice('a', false)], 'a');

    openReleaseNoticesPanel();

    expect(openSettingsSubview).toHaveBeenCalledWith('release-notices');
    expect(releaseNoticeModalOpen()).toBe(false);
    expect(resolveReleaseNotice).not.toHaveBeenCalled();
    expect(owedReleaseNotice()?.id).toBe('a');
  });
});

describe('the action a notice row draws', () => {
  it('is the notice\'s own label', () => {
    expect(releaseNoticeActionLabel(notice('a', false, true))).toBe('Do it');
  });

  it('is nothing for a notice with no action', () => {
    expect(releaseNoticeActionLabel(notice('a', false))).toBeNull();
  });

  // A workspace that skipped releases owes several audit notices. The engine
  // marks all but the last as deferred, so the reader runs one audit.
  it('is nothing when a later owed notice carries the same action', () => {
    expect(releaseNoticeActionLabel(notice('a', false, true, true))).toBeNull();
  });

  // Got it says "read", never "done", so an answered row keeps its action.
  it('comes back once the notice is answered', () => {
    expect(releaseNoticeActionLabel(notice('a', true, true, true))).toBe('Do it');
  });
});
