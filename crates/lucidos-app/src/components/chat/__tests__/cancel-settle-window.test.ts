import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  armCancelSettle,
  isCancelSettling,
  computePromptEscapeAction,
  CANCEL_SETTLE_MS,
} from '../prompt-input-helpers';

describe('post-submit cancel settle window', () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.runOnlyPendingTimers(); vi.useRealTimers(); });

  it('is not settling before any submit — a fresh question keeps an immediately-usable Cancel', () => {
    expect(isCancelSettling(1_000)).toBe(false);
  });

  it('settles the instant a constructive submit fires and clears after the window', () => {
    armCancelSettle(1_000);
    expect(isCancelSettling(1_000)).toBe(true);
    expect(isCancelSettling(1_000 + CANCEL_SETTLE_MS - 1)).toBe(true);
    expect(isCancelSettling(1_000 + CANCEL_SETTLE_MS)).toBe(false);
  });

  it('re-arming a later submit extends the window from that latest submit', () => {
    armCancelSettle(1_000);
    armCancelSettle(1_500);
    expect(isCancelSettling(1_000 + CANCEL_SETTLE_MS + 1)).toBe(true);
    expect(isCancelSettling(1_500 + CANCEL_SETTLE_MS)).toBe(false);
  });

  it('the live timer re-enables the button by flipping the signal back off', () => {
    armCancelSettle(Date.now());
    expect(isCancelSettling()).toBe(true);
    vi.advanceTimersByTime(CANCEL_SETTLE_MS);
    expect(isCancelSettling()).toBe(false);
  });
});

describe('what Escape does in the prompt', () => {
  it('cancels when there is a live turn or a pending card to cancel', () => {
    expect(computePromptEscapeAction(true, false, false)).toBe('cancel');
  });

  it('blurs when there is nothing to cancel, as it always did', () => {
    expect(computePromptEscapeAction(false, false, false)).toBe('blur');
  });

  it('does nothing during the settle window, so Enter then Escape cannot abort the new turn', () => {
    expect(computePromptEscapeAction(true, true, false)).toBe('ignore');
  });

  it('still blurs during the settle window when there is nothing to cancel', () => {
    expect(computePromptEscapeAction(false, true, false)).toBe('blur');
  });

  it('leaves side-question mode first, before it would stop a turn', () => {
    expect(computePromptEscapeAction(true, false, true)).toBe('leave-side-question');
    expect(computePromptEscapeAction(true, true, true)).toBe('leave-side-question');
    expect(computePromptEscapeAction(false, false, true)).toBe('leave-side-question');
  });
});
