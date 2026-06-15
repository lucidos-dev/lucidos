import { describe, it, expect, beforeEach, vi } from 'vitest';
import { clearQueuedUploadSend, computeMorphMode, dispatchSend, queueUploadSend, queuedUploadSends, submittingThreadIds, takeQueuedUploadSend } from '../PromptInput';

const base = {
  hasContent: false,
  cancelTargetId: null as string | null,
  isCanceling: false,
  hasBannerOrSectionButtons: false,
};

describe('computeMorphMode', () => {
  it('placeholder when nothing else owns the slot', () => {
    expect(computeMorphMode(base)).toBe('placeholder');
  });

  it('hidden when banner or section buttons own the slot', () => {
    expect(computeMorphMode({ ...base, hasBannerOrSectionButtons: true })).toBe('hidden');
  });

  it('send when user typed text and nothing blocks the turn', () => {
    expect(computeMorphMode({ ...base, hasContent: true })).toBe('send');
  });

  it('cancel when a cancel target exists and slot is otherwise free', () => {
    expect(computeMorphMode({ ...base, cancelTargetId: 't1' })).toBe('cancel');
  });

  it('canceling when cancel target exists and click already fired', () => {
    expect(computeMorphMode({ ...base, cancelTargetId: 't1', isCanceling: true })).toBe('canceling');
  });

  // hasContent wins over cancelTargetId — once the user starts typing, the
  // morph flips back to Send so the follow-up path stays accessible.
  it('send wins over cancel when user has typed text', () => {
    expect(computeMorphMode({ ...base, hasContent: true, cancelTargetId: 't1' })).toBe('send');
  });

  // Documents the intended resolution sequence on Send. The actual ordering
  // invariant (stamp before send) is enforced by dispatchSend's tests below.
  it('Send→Cancel resolves through send → cancel without hitting hidden', () => {
    // 1. User taps Send: hasContent true, no cancel target yet
    expect(computeMorphMode({ ...base, hasContent: true })).toBe('send');
    // 2. dispatchSend stamps cancelTargetId before invoking the send call
    expect(computeMorphMode({ ...base, hasContent: true, cancelTargetId: 't1' })).toBe('send');
    // 3. sendCompose runs: hasContent flips false, section buttons appear
    expect(computeMorphMode({
      ...base,
      hasContent: false,
      cancelTargetId: 't1',
      hasBannerOrSectionButtons: true,
    })).toBe('cancel');
  });
});

// Locks dispatchSend's stamp-before-send invariant — see the helper's
// docstring in PromptInput.tsx for the why.
describe('dispatchSend ordering', () => {
  beforeEach(() => {
    submittingThreadIds.value = new Set();
  });

  it('stamps threadId in submittingThreadIds before invoking send', () => {
    let stampedAtSendTime = false;
    const send = vi.fn(() => {
      stampedAtSendTime = submittingThreadIds.value.has('t1');
      return Promise.resolve();
    });

    const { submittedId } = dispatchSend('t1', send);

    expect(send).toHaveBeenCalledOnce();
    expect(stampedAtSendTime).toBe(true);
    expect(submittedId).toBe('t1');
  });

  it('does not pre-stamp for raw new sends (threadId null)', () => {
    let stampedAtSendTime = false;
    const send = vi.fn(() => {
      stampedAtSendTime = submittingThreadIds.value.size > 0;
      return Promise.resolve();
    });

    dispatchSend(null, send);

    expect(stampedAtSendTime).toBe(false);
  });
});

describe('queued upload sends', () => {
  beforeEach(() => {
    queuedUploadSends.value = new Map();
    submittingThreadIds.value = new Set();
  });

  it('stores the latest send intent for a thread until upload settlement consumes it', () => {
    queueUploadSend('t1', { useClaudeCode: false, context: { app_context: { app_id: 'a' } } });
    queueUploadSend('t1', { useClaudeCode: true, context: null });

    const intent = takeQueuedUploadSend('t1');

    expect(intent).toEqual({ useClaudeCode: true, context: null });
    expect(queuedUploadSends.value.has('t1')).toBe(false);
  });

  it('marks a queued upload send as submitting so the prompt can morph to Cancel', () => {
    queueUploadSend('t1', { useClaudeCode: false, context: null });

    expect(submittingThreadIds.value.has('t1')).toBe(true);
    expect(computeMorphMode({ ...base, hasContent: false, cancelTargetId: 't1' })).toBe('cancel');
  });

  it('clearing a queued upload send also clears its optimistic submitting state', () => {
    queueUploadSend('t1', { useClaudeCode: false, context: null });

    clearQueuedUploadSend('t1');

    expect(queuedUploadSends.value.has('t1')).toBe(false);
    expect(submittingThreadIds.value.has('t1')).toBe(false);
  });
});
