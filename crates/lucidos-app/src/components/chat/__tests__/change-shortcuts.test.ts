/** The Diff and Apply shortcuts act only where their buttons would.
 *
 *  Both read the banner's own state. So an apply already in flight, an archive
 *  under way, or a thread with no diff leaves the keystroke doing nothing.
 *
 *  Plan: `docs/plans/2026-10-01-shortcuts-for-every-toggle.md`. */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
  focusThread: vi.fn(),
}));
vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));
vi.mock('../../../store/actions/threadActions', () => ({
  resolveThreadActions: vi.fn(() => []),
  threadHasIncompleteChange: () => false,
}));

import { applyFocusedThreadChange, showFocusedThreadDiff } from '../WaitingBanner';
import { resolveThreadActions, type TaggedAction } from '../../../store/actions/threadActions';
import { viewThreadCcDiff } from '../../../store/actions/repositories';
import { setDraft } from '../../../store/composeDrafts';
import {
  applyingNowThreadIds,
  discardingCCThreadIds,
  archivingThreadIds,
  armingStandingApplyThreadIds,
  focusedThreadId,
  threadMap,
} from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { ThreadMeta } from '../../../store/thread-events';

const ID = 't1';

function focus(meta: Partial<ThreadMeta>): void {
  threadMap.value = new Map([[ID, makeThreadState(ID, { meta: { channel: 'claude_code', ...meta } })]]);
  focusedThreadId.value = ID;
}

type Invoke = ReturnType<typeof vi.fn<() => void>>;

function offer(...kinds: TaggedAction['kind'][]): Record<string, Invoke> {
  const invokes: Record<string, Invoke> = {};
  vi.mocked(resolveThreadActions).mockReturnValue(kinds.map((kind): TaggedAction => {
    invokes[kind] = vi.fn<() => void>();
    return { kind, category: 'primary', label: kind, invoke: invokes[kind] };
  }));
  return invokes;
}

beforeEach(() => {
  vi.clearAllMocks();
  applyingNowThreadIds.value = new Map();
  discardingCCThreadIds.value = new Set();
  archivingThreadIds.value = new Set();
  setDraft(ID, { text: '', image_hashes: [], mode: null });
  armingStandingApplyThreadIds.value = new Set();
});

describe('the Apply shortcut', () => {
  it("applies a settled thread's pending change", () => {
    focus({ status: 'idle' });
    const invokes = offer('apply', 'discard');
    applyFocusedThreadChange();
    expect(invokes.apply).toHaveBeenCalledTimes(1);
    expect(invokes.discard).not.toHaveBeenCalled();
  });

  it('does nothing while that apply is already in flight', () => {
    focus({ status: 'idle' });
    applyingNowThreadIds.value = new Map([[ID, 'requesting']]);
    const invokes = offer('apply');
    applyFocusedThreadChange();
    expect(invokes.apply).not.toHaveBeenCalled();
  });

  it('does nothing while the thread is archiving', () => {
    focus({ status: 'idle' });
    archivingThreadIds.value = new Set([ID]);
    const invokes = offer('apply');
    applyFocusedThreadChange();
    expect(invokes.apply).not.toHaveBeenCalled();
  });

  it('does nothing with a draft in the composer, where Send hides Apply', () => {
    focus({ status: 'idle' });
    setDraft(ID, { text: 'a follow-up', image_hashes: [], mode: null });
    const invokes = offer('apply');
    applyFocusedThreadChange();
    expect(invokes.apply).not.toHaveBeenCalled();
  });

  it('arms the standing apply on a thread still working', () => {
    focus({ status: 'running' });
    const invokes = offer('apply_when_settled');
    applyFocusedThreadChange();
    expect(invokes.apply_when_settled).toHaveBeenCalledTimes(1);
  });

  it('drops a press while the standing apply is already arming, as the button does', () => {
    focus({ status: 'running' });
    armingStandingApplyThreadIds.value = new Set([ID]);
    const invokes = offer('apply_when_settled');
    applyFocusedThreadChange();
    expect(invokes.apply_when_settled).not.toHaveBeenCalled();
  });
});

describe('the Diff shortcut', () => {
  it("opens the focused thread's diff when the branch has one", () => {
    focus({ codingAgentHasDiff: true });
    showFocusedThreadDiff();
    expect(viewThreadCcDiff).toHaveBeenCalledWith(ID);
  });

  it('does nothing while a discard is in flight, where the banner shows only its spinner', () => {
    focus({ status: 'idle', codingAgentHasDiff: true });
    discardingCCThreadIds.value = new Set([ID]);
    showFocusedThreadDiff();
    expect(viewThreadCcDiff).not.toHaveBeenCalled();
  });

  it('still opens it with a draft in the composer, where the standalone Diff shows', () => {
    focus({ status: 'idle', codingAgentHasDiff: true });
    discardingCCThreadIds.value = new Set([ID]);
    setDraft(ID, { text: 'a follow-up', image_hashes: [], mode: null });
    showFocusedThreadDiff();
    expect(viewThreadCcDiff).toHaveBeenCalledWith(ID);
  });

  it('does nothing when there is no diff, where no Diff button shows', () => {
    focus({ codingAgentHasDiff: false });
    showFocusedThreadDiff();
    expect(viewThreadCcDiff).not.toHaveBeenCalled();
  });
});
