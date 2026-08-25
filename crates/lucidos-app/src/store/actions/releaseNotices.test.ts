import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ReleaseNotice, ReleaseNoticeView } from '../../api/client';

const resolveReleaseNotice = vi.fn();
const sendSeededPrompt = vi.fn();

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  releaseNotices: vi.fn(),
  resolveReleaseNotice: (...a: unknown[]) => resolveReleaseNotice(...a),
}));

vi.mock('./compose', () => ({
  sendSeededPrompt: (...a: unknown[]) => sendSeededPrompt(...a),
}));

const {
  acknowledgeReleaseNotice,
  dismissReleaseNoticeModal,
  owedReleaseNotice,
  owedReleaseNoticeCount,
  releaseNoticeDismissed,
  releaseNoticeModalOpen,
  takeReleaseNoticeAction,
} = await import('./releaseNotices');
const { releaseNoticeView } = await import('../store');

function notice(id: string, resolved: boolean, action = false): ReleaseNotice {
  return {
    id,
    since: '2.0.0',
    title: `Notice ${id}`,
    body: 'Do the thing.',
    resolved,
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

  it('counts what is still owed, so the modal can say 1 of 3', () => {
    loaded([notice('a', true), notice('b', false), notice('c', false)], 'b');
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

  it('steps to the next notice without closing the modal', async () => {
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

  it('sends the notice sentence, then closes and answers', async () => {
    loaded([notice('a', false, true), notice('b', false)], 'a');
    sendSeededPrompt.mockResolvedValue(true);
    resolveReleaseNotice.mockResolvedValue(
      settled([notice('a', true, true), notice('b', false)], 'b'),
    );

    await takeReleaseNoticeAction(notice('a', false, true));

    expect(sendSeededPrompt).toHaveBeenCalledWith('Do the thing for me.', expect.any(String));
    expect(resolveReleaseNotice).toHaveBeenCalledWith('a');
    // Closed even though 'b' is now owed: the send landed the reader in a new
    // thread, and an overlay would cover it.
    expect(releaseNoticeDismissed.value).toBe(true);
    expect(releaseNoticeModalOpen()).toBe(false);
  });

  it('answers nothing when the send did not happen', async () => {
    // Declining the draft-override confirm starts nothing, so the notice is
    // still owed and the modal stays up to ask again.
    loaded([notice('a', false, true)], 'a');
    sendSeededPrompt.mockResolvedValue(false);

    await takeReleaseNoticeAction(notice('a', false, true));

    expect(resolveReleaseNotice).not.toHaveBeenCalled();
    expect(releaseNoticeDismissed.value).toBe(false);
    expect(releaseNoticeModalOpen()).toBe(true);
  });
});

describe('dismissing without answering', () => {
  beforeEach(() => {
    releaseNoticeDismissed.value = false;
    releaseNoticeView.value = { status: 'not-loaded' };
    vi.clearAllMocks();
  });

  // Escape, the X and an outside click all land here.
  it('closes the modal and resolves nothing, so the notice returns', () => {
    loaded([notice('a', false)], 'a');

    dismissReleaseNoticeModal();

    expect(resolveReleaseNotice).not.toHaveBeenCalled();
    expect(releaseNoticeModalOpen()).toBe(false);
    // Still owed. A reload asks again, which is what "not now" means.
    expect(owedReleaseNotice()?.id).toBe('a');
  });
});
