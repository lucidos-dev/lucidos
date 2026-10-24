import { describe, it, expect, beforeAll } from 'vitest';

// Must be set BEFORE the store loads: `diffSideBySide` reads its key at import
// time, the way `filePreviewSource` does. This is the half a plain
// write-then-read test would miss, and the half that makes the preference
// survive a reload at all.
localStorage.setItem('lucidos-diff-side-by-side', 'true');

let store: typeof import('../store');

beforeAll(async () => {
  store = await import('../store');
  await import('../effects');
});

/** Side by side is a way of READING diffs, not a per-file override like
 *  `diffWholeFile` (which store/effects.ts deliberately resets whenever the
 *  previewed file changes). So it persists across files and across reloads. */
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

  // The per-file reset in store/effects.ts clears `diffWholeFile` on every new
  // previewed path. Sweeping this signal in with it would drop the user's
  // choice each time they clicked through the changed-files sidebar.
  it('survives the previewed file changing', () => {
    store.diffSideBySide.value = true;
    store.panelOverlay.value = { type: 'file-preview', path: 'artifacts/a.md' };
    store.panelOverlay.value = { type: 'file-preview', path: 'artifacts/b.md' };
    expect(store.diffSideBySide.value).toBe(true);
  });
});
