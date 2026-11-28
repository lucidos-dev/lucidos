import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { preferences } from '../store';
import { applyStyleOverrides } from './preferences';

const platformMocks = vi.hoisted(() => ({ isIOS: false, isTauri: false, isIOSPwa: false }));
vi.mock('../../utils/platform', () => ({
  isIOS: () => platformMocks.isIOS,
  isTauri: () => platformMocks.isTauri,
  isIOSPwa: () => platformMocks.isIOSPwa,
}));
vi.mock('../../utils/tauri', () => ({
  setTitlebarColor: vi.fn(() => Promise.resolve()),
  windowReadyToShow: vi.fn(),
}));

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
    expect(inlineProps).not.toHaveProperty('--accent');
  });

  it('removes a name whose NEW value is invalid, rather than leaving the old one stuck', () => {
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
    applyStyleOverrides({ '--accent': '#ff0000' });
    expect(JSON.parse(localStorage.getItem('lucidos-style-overrides')!)).toEqual({ '--accent': '#ff0000' });
  });
});
