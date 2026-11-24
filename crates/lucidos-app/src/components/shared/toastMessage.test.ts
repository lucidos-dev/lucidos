import { describe, it, expect } from 'vitest';
import { clampToastText, ERROR_MAX_CHARS, TOAST_MAX_CHARS } from './toastMessage';

describe('clampToastText', () => {
  it('leaves an ordinary message untouched', () => {
    expect(clampToastText('Backup complete', 'success')).toBe('Backup complete');
    expect(clampToastText('Compose sync failed: 410 thread discarded', 'error'))
      .toBe('Compose sync failed: 410 thread discarded');
  });

  it('keeps the line breaks of any other kind', () => {
    const message = 'No release candidate tonight.\n\n• Alpha\n• Beta';
    expect(clampToastText(message, 'info')).toBe(message);
  });

  it('flattens an error to one line', () => {
    expect(clampToastText('Failed\n• first\n• second', 'error')).toBe('Failed • first • second');
  });

  it('clamps a long error and marks the cut with an ellipsis', () => {
    const out = clampToastText(`Sync failed: ${'detail '.repeat(200)}`, 'error');
    expect(out.length).toBeLessThanOrEqual(ERROR_MAX_CHARS);
    expect(out.endsWith('…')).toBe(true);
    expect(out.startsWith('Sync failed: ')).toBe(true);
  });

  it('bounds every other kind too, for the payloads nobody sized', () => {
    const out = clampToastText('x'.repeat(50_000), 'info');
    expect(out.length).toBeLessThanOrEqual(TOAST_MAX_CHARS);
    expect(out.endsWith('…')).toBe(true);
  });

  it('never cuts a code point in half', () => {
    const out = clampToastText('🙂'.repeat(300), 'error');
    expect([...out].every((c) => c === '🙂' || c === '…')).toBe(true);
  });
});
