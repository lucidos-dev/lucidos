import { describe, it, expect, beforeAll } from 'vitest';

localStorage.setItem('lucidos-diff-side-by-side', 'true');

let store: typeof import('../store');

beforeAll(async () => {
  store = await import('../store');
  await import('../effects');
});

describe('diffSideBySide is a persisted preference', () => {
  it('restores the prior session\'s choice at import time', () => {
    expect(store.diffSideBySide.value).toBe(true);
  });

  it('writes the choice back so the next session restores it', () => {
    store.diffSideBySide.value = false;
    expect(localStorage.getItem('lucidos-diff-side-by-side')).toBe('false');
    store.diffSideBySide.value = true;
    expect(localStorage.getItem('lucidos-diff-side-by-side')).toBe('true');
  });

  it('survives the previewed file changing', () => {
    store.diffSideBySide.value = true;
    store.panelOverlay.value = { type: 'file-preview', path: 'artifacts/a.md' };
    store.panelOverlay.value = { type: 'file-preview', path: 'artifacts/b.md' };
    expect(store.diffSideBySide.value).toBe(true);
  });
});
