import { describe, it, expect, beforeEach } from 'vitest';
import {
  clampSidebarRem, sidebarMaxRem, readStoredSidebarRem, setSidebarRem, repoPreviewSidebarRem,
  SIDEBAR_DEFAULT_REM, SIDEBAR_MIN_REM, DIFF_PANE_MIN_REM, SIDEBAR_WIDTH_KEY,
} from './repoPreviewSidebarWidth';

describe('clampSidebarRem', () => {
  const wide = 100;

  it('keeps a width that fits', () => {
    expect(clampSidebarRem(30, wide)).toBe(30);
  });

  it('never goes below the minimum, so the sidebar cannot collapse', () => {
    expect(clampSidebarRem(2, wide)).toBe(SIDEBAR_MIN_REM);
  });

  it('leaves the diff pane its minimum', () => {
    expect(clampSidebarRem(95, wide)).toBe(wide - DIFF_PANE_MIN_REM);
  });

  it('re-clamps against a narrower container', () => {
    expect(clampSidebarRem(40, 55)).toBe(55 - DIFF_PANE_MIN_REM);
    expect(clampSidebarRem(40, wide)).toBe(40);
  });

  it('keeps the minimum when the container cannot hold both panes', () => {
    expect(sidebarMaxRem(20)).toBe(SIDEBAR_MIN_REM);
    expect(clampSidebarRem(30, 20)).toBe(SIDEBAR_MIN_REM);
  });

  it('clamps only to the minimum while the container is unmeasured', () => {
    expect(clampSidebarRem(40, 0)).toBe(40);
    expect(clampSidebarRem(2, 0)).toBe(SIDEBAR_MIN_REM);
  });
});

describe('the persisted width', () => {
  beforeEach(() => localStorage.clear());

  it('defaults when nothing is stored', () => {
    expect(readStoredSidebarRem()).toBe(SIDEBAR_DEFAULT_REM);
  });

  it('defaults on garbage rather than rendering NaN', () => {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, 'wide');
    expect(readStoredSidebarRem()).toBe(SIDEBAR_DEFAULT_REM);
  });

  it('reads back what was stored, floored at the minimum', () => {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, '28.5');
    expect(readStoredSidebarRem()).toBe(28.5);
    localStorage.setItem(SIDEBAR_WIDTH_KEY, '3');
    expect(readStoredSidebarRem()).toBe(SIDEBAR_MIN_REM);
  });

  it('writes the signal and localStorage together', () => {
    setSidebarRem(22);
    expect(repoPreviewSidebarRem.value).toBe(22);
    expect(localStorage.getItem(SIDEBAR_WIDTH_KEY)).toBe('22');
  });
});
