/**
 * `showToast` bounds the message it STORES, not the one the renderer draws.
 *
 * The clamp itself is unit-tested next to the parse contract it protects
 * (`components/shared/toastMessage.test.ts`). What is under test here is that
 * `showToast` is the one gate, so no caller can route around it: a keyed update
 * writes through a second branch, and it went unclamped in the first draft.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { showToast, toasts } from '../store';

/** The gateway's 503 boot splash, in the shape that reached the screen: a
 *  prefix from the caller, then a whole HTML page. */
const HTML_ERROR = [
  'Compose sync failed: 503 <!doctype html><html><head>',
  '<meta http-equiv="refresh" content="2">',
  '<meta name="theme-color" content="#0a4ea8">',
  '</head></html>',
].join('\n');

describe('showToast bounds every message it stores', () => {
  beforeEach(() => { toasts.value = []; });

  it('flattens and clamps an error, whatever the caller handed it', () => {
    showToast(HTML_ERROR, 'error');

    const [toast] = toasts.value;
    expect(toast.message).not.toContain('\n');
    expect(toast.message.length).toBeLessThanOrEqual(200);
    // Still names what failed: the clamp cuts the tail, never the lead.
    expect(toast.message.startsWith('Compose sync failed: 503')).toBe(true);
  });

  it('clamps the KEYED in-place update too', () => {
    showToast('Compose sync failed: 503', 'error', { key: 'compose-sync-rejected' });
    showToast(HTML_ERROR, 'error', { key: 'compose-sync-rejected' });

    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).not.toContain('\n');
    expect(toasts.value[0].message.length).toBeLessThanOrEqual(200);
  });

  it('leaves a structured status message alone', () => {
    const message = '2 changes ready to apply\n• Alpha\n• Beta';
    showToast(message, 'info');
    expect(toasts.value[0].message).toBe(message);
  });
});
