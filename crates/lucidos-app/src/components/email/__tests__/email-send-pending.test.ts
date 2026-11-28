import { describe, it, expect } from 'vitest';
import { driveSend } from '../EmailConfirmModal';
import { EMAIL_SEND_TIMEOUT_MS } from '../../../api/client/settings';


type Result = { success: boolean; error?: string };

function harness(send: () => Promise<Result>, panelGone = false) {
  let sending = false;
  const toasts: Array<{ msg: string; type: string }> = [];
  let marked = 0;
  return {
    toasts,
    isMarkedSent: () => marked > 0,
    isSending: () => sending,
    run: () =>
      driveSend({
        isSending: () => sending,
        setSending: (v: boolean) => { sending = v; },
        send,
        toast: (msg: string, type: 'success' | 'error') => { toasts.push({ msg, type }); },
        markSent: () => { marked++; return !panelGone; },
      }),
  };
}

describe('driveSend', () => {
  it('guards against double-submit: a click while a send is in flight is a no-op', async () => {
    let calls = 0;
    let resolve!: (r: Result) => void;
    const h = harness(() => { calls++; return new Promise<Result>((r) => { resolve = r; }); });

    const first = h.run();
    expect(h.isSending()).toBe(true);
    await h.run();
    await h.run();
    expect(calls).toBe(1);

    resolve({ success: true });
    await first;
    expect(calls).toBe(1);
  });

  it('success → panel becomes the sent receipt in place, no toast, pending cleared', async () => {
    const h = harness(() => Promise.resolve({ success: true }));
    await h.run();
    expect(h.isMarkedSent()).toBe(true);
    expect(h.toasts).toEqual([]);
    expect(h.isSending()).toBe(false);
  });

  it('success after the user dismissed the panel → falls back to the success toast', async () => {
    const h = harness(() => Promise.resolve({ success: true }), true);
    await h.run();
    expect(h.isMarkedSent()).toBe(true);
    expect(h.toasts).toEqual([{ msg: 'Email sent successfully', type: 'success' }]);
    expect(h.isSending()).toBe(false);
  });

  it('engine {success:false} → error toast carries the engine message, form stays open for retry', async () => {
    const h = harness(() =>
      Promise.resolve({ success: false, error: 'SMTP send via smtp.example.com:587 timed out after 80s' }));
    await h.run();
    expect(h.toasts[0].type).toBe('error');
    expect(h.toasts[0].msg).toContain('smtp.example.com:587');
    expect(h.isMarkedSent()).toBe(false);
    expect(h.isSending()).toBe(false);
  });

  it('thrown transport error → error toast, form stays open, pending cleared', async () => {
    const h = harness(() => Promise.reject(new Error('Load failed')));
    await h.run();
    expect(h.toasts[0].type).toBe('error');
    expect(h.toasts[0].msg).toContain('Load failed');
    expect(h.isMarkedSent()).toBe(false);
    expect(h.isSending()).toBe(false);
  });
});

describe('EMAIL_SEND_TIMEOUT_MS', () => {
  it('stays above the engine-side worst case (30s OAuth refresh + 80s SMTP) so the engine error surfaces, not a client timeout', () => {
    expect(EMAIL_SEND_TIMEOUT_MS).toBeGreaterThan(110_000);
  });
});
