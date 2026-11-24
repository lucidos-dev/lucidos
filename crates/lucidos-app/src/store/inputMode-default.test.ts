import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

describe('inputMode persistence (compose toggle remembers last choice)', () => {
  beforeEach(() => {
    vi.resetModules();
    localStorage.removeItem('lucidos-input-mode');
    localStorage.removeItem('lucidos-input-target');
  });

  afterEach(() => {
    localStorage.removeItem('lucidos-input-mode');
    localStorage.removeItem('lucidos-input-target');
  });

  it('initializes to { type: do } when nothing is stored', async () => {
    const { inputMode } = await import('./store');
    expect(inputMode.value).toEqual({ type: 'do' });
  });

  it('restores { type: coding_agent } from localStorage on init', async () => {
    localStorage.setItem('lucidos-input-mode', JSON.stringify({ type: 'coding_agent' }));
    const { inputMode } = await import('./store');
    expect(inputMode.value).toEqual({ type: 'coding_agent' });
  });

  it('migrates the legacy { type: claude_code } payload to { type: coding_agent }', async () => {
    localStorage.setItem('lucidos-input-mode', JSON.stringify({ type: 'claude_code' }));
    const { inputMode } = await import('./store');
    expect(inputMode.value).toEqual({ type: 'coding_agent' });
  });

  it('restores { type: do } from localStorage on init', async () => {
    localStorage.setItem('lucidos-input-mode', JSON.stringify({ type: 'do' }));
    const { inputMode } = await import('./store');
    expect(inputMode.value).toEqual({ type: 'do' });
  });

  it('treats a malformed localStorage payload as { type: do } instead of throwing', async () => {
    localStorage.setItem('lucidos-input-mode', 'not-json');
    const { inputMode } = await import('./store');
    expect(inputMode.value).toEqual({ type: 'do' });
  });

  it('treats an unknown type in localStorage as { type: do }', async () => {
    localStorage.setItem('lucidos-input-mode', JSON.stringify({ type: 'something-else' }));
    const { inputMode } = await import('./store');
    expect(inputMode.value).toEqual({ type: 'do' });
  });

  it('ignores the legacy lucidos-input-target key (only the current shape is read)', async () => {
    localStorage.setItem('lucidos-input-target', 'claude_code');
    const { inputMode } = await import('./store');
    expect(inputMode.value).toEqual({ type: 'do' });
  });

  it('clears the legacy lucidos-input-target key on effects.ts load', async () => {
    localStorage.setItem('lucidos-input-target', 'claude_code');
    await import('./store');
    await import('./effects');
    expect(localStorage.getItem('lucidos-input-target')).toBeNull();
  });

  it('changing inputMode persists to localStorage so the next page load picks it up', async () => {
    const { inputMode } = await import('./store');
    await import('./effects');
    inputMode.value = { type: 'coding_agent' };
    await Promise.resolve();
    expect(localStorage.getItem('lucidos-input-mode')).toBe(JSON.stringify({ type: 'coding_agent' }));
  });

  it('switching back to lucidos also persists', async () => {
    localStorage.setItem('lucidos-input-mode', JSON.stringify({ type: 'coding_agent' }));
    const { inputMode } = await import('./store');
    await import('./effects');
    inputMode.value = { type: 'do' };
    await Promise.resolve();
    expect(localStorage.getItem('lucidos-input-mode')).toBe(JSON.stringify({ type: 'do' }));
  });
});
