import { describe, it, expect, beforeEach } from 'vitest';
import { showToast, toasts } from '../store';

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
    expect(toast.message.startsWith('Compose sync failed: 503')).toBe(true);
  });

  it('clamps the KEYED in-place update too', () => {
    showToast('Compose sync failed: 503', 'error', { key: 'compose-sync-rejected' });
    showToast(HTML_ERROR, 'error', { key: 'compose-sync-rejected' });

    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).not.toContain('\n');
    expect(toasts.value[0].message.length).toBeLessThanOrEqual(200);
  });

  it('leaves a multi-line status message alone', () => {
    const message = '• Alpha\n• Beta';
    showToast(message, 'info', { title: '2 changes ready to apply' });
    expect(toasts.value[0].title).toBe('2 changes ready to apply');
    expect(toasts.value[0].message).toBe(message);
  });

  it('stores a blank title as no title', () => {
    showToast('Saved', 'info', { title: '   ' });
    expect(toasts.value[0].title).toBeUndefined();
  });

  it('flattens an error title as well as its message', () => {
    showToast('Could not reach it', 'error', { title: 'Sync\nfailed' });
    expect(toasts.value[0].title).toBe('Sync failed');
  });
});

describe('a keyed re-show owns the title too', () => {
  beforeEach(() => { toasts.value = []; });

  it('replaces the title, and clears it when the new show has none', () => {
    showToast('Waiting for the network', 'info', { key: 'serve', title: 'Exposing' });
    showToast('Still waiting', 'info', { key: 'serve', title: 'Exposing again' });
    expect(toasts.value[0].title).toBe('Exposing again');

    showToast('Exposed', 'success', { key: 'serve' });
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].title).toBeUndefined();
    expect(toasts.value[0].message).toBe('Exposed');
  });
});
