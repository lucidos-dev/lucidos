import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  armCancelSettle,
  isCancelSettling,
  computePromptEscapeAction,
  CANCEL_SETTLE_MS,
} from '../prompt-input-helpers';

// Regression for the iOS-PWA accidental-abort bug (thread fe390597): the
// prompt-row Send/Submit button morphs IN PLACE into a destructive Cancel/Stop
// the instant the user submits (a normal Send → running-turn Stop via the
// optimistic submitting flag; a typed answer's Submit → lone Cancel once the
// draft clears). On a laggy device the user taps the same spot several times
// before the UI catches up, so a queued repeat tap landed on the freshly-morphed
// Cancel and aborted the turn they just started — stamping the next pending
// question `Canceled` + `ResponseCanceled { user_stop }`. After a constructive
// submit we now hold the destructive morph DISABLED for a short settle window so
// the burst is absorbed; isCancelSettling() drives both the disabled prop and the
// onClick short-circuit (via cancelExchangeForTarget).
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
    // The first window (ends 2_200) would have expired, but the second (ends
    // 2_700) keeps it settling.
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

// Escape inside the composer is the keyboard twin of the row's red button, so
// it inherits the same hazard the settle window exists for: Enter sends, and the
// reflex Escape a beat later would abort the turn that Enter just started. It
// reaches the textarea only when no overlay is open (the overlay stack handles
// Escape first, in the capture phase, and stops propagation).
describe('what Escape does in the prompt', () => {
  it('cancels when there is a live turn or a pending card to cancel', () => {
    expect(computePromptEscapeAction(true, false)).toBe('cancel');
  });

  it('blurs when there is nothing to cancel, as it always did', () => {
    expect(computePromptEscapeAction(false, false)).toBe('blur');
  });

  it('does nothing during the settle window, so Enter then Escape cannot abort the new turn', () => {
    expect(computePromptEscapeAction(true, true)).toBe('ignore');
  });

  it('still blurs during the settle window when there is nothing to cancel', () => {
    expect(computePromptEscapeAction(false, true)).toBe('blur');
  });
});
