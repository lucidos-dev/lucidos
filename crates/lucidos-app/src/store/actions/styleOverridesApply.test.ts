/**
 * Applying the live style remote's overrides onto `<html>`.
 *
 * Both cases below are regressions from the review of the change that added
 * this, and both are about the same thing: an inline custom property OUTLIVES
 * the map that set it, so "stop applying X" is a `removeProperty` call someone
 * has to make, and "reset" is a write that itself announces a change.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { preferences } from '../store';
import { applyStyleOverrides } from './preferences';

const platformMocks = vi.hoisted(() => ({ isIOS: false, isTauri: false, isIOSPwa: false }));
vi.mock('../../utils/platform', () => ({
  isIOS: () => platformMocks.isIOS,
  isTauri: () => platformMocks.isTauri,
  isIOSPwa: () => platformMocks.isIOSPwa,
}));
vi.mock('../../utils/tauri', () => ({ setTitlebarColor: vi.fn(() => Promise.resolve()) }));

describe('applyStyleOverrides', () => {
  let inlineProps: Record<string, string>;
  let originalStyle: unknown;

  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
    inlineProps = {};
    const el = document.documentElement as unknown as Record<string, unknown>;
    originalStyle = el.style;
    el.style = {
      setProperty: (k: string, v: string) => { inlineProps[k] = v; },
      getPropertyValue: (k: string) => inlineProps[k] ?? '',
      removeProperty: (k: string) => { delete inlineProps[k]; },
    };
    // Start from a clean applied-set: the module tracks what it wrote across
    // calls, and an earlier suite may have left names behind.
    applyStyleOverrides({});
    inlineProps = {};
  });

  afterEach(() => {
    applyStyleOverrides({});
    (document.documentElement as unknown as Record<string, unknown>).style = originalStyle;
  });

  it('applies a valid map', () => {
    applyStyleOverrides({ '--accent': '#ff0000', '--space-sm': '0.75rem' });
    expect(inlineProps['--accent']).toBe('#ff0000');
    expect(inlineProps['--space-sm']).toBe('0.75rem');
  });

  it('removes a name dropped from the map', () => {
    applyStyleOverrides({ '--accent': '#ff0000' });
    applyStyleOverrides({});
    // Not merely unset in the map: actually removed from the element, or the
    // old value keeps painting until the next full reload.
    expect(inlineProps).not.toHaveProperty('--accent');
  });

  it('removes a name whose NEW value is invalid, rather than leaving the old one stuck', () => {
    // The regression: keying the removal loop on the incoming map meant a name
    // whose replacement failed validation was skipped by both loops, so it kept
    // painting its previous value, from a function that promises to validate.
    applyStyleOverrides({ '--accent': '#ff0000' });
    applyStyleOverrides({ '--accent': 'url(http://evil.test/x.png)' });
    expect(inlineProps).not.toHaveProperty('--accent');
  });

  it('drops an invalid entry without disturbing the valid ones', () => {
    applyStyleOverrides({ '--accent': '#ff0000', '--bad': 'red; position: fixed' });
    expect(inlineProps['--accent']).toBe('#ff0000');
    expect(inlineProps).not.toHaveProperty('--bad');
  });

  it('mirrors the map to localStorage for the next first paint', () => {
    // The boot script reads this key before any module runs, so a tuned value
    // paints on the first frame instead of flashing the untuned one.
    applyStyleOverrides({ '--accent': '#ff0000' });
    expect(JSON.parse(localStorage.getItem('lucidos-style-overrides')!)).toEqual({ '--accent': '#ff0000' });
  });
});
