import { describe, it, expect } from 'vitest';
import {
  isValidOverrideName, isValidOverrideValue, parseStyleOverrides,
  serializeStyleOverrides, styleResetRequested,
  MAX_STYLE_OVERRIDES, MAX_STYLE_VALUE_LENGTH,
} from './styleOverrides';

describe('override names', () => {
  it('accepts a custom property', () => {
    expect(isValidOverrideName('--accent')).toBe(true);
    expect(isValidOverrideName('--font-size-2xs')).toBe(true);
  });

  it('rejects anything that is not a custom property', () => {
    expect(isValidOverrideName('color')).toBe(false);
    expect(isValidOverrideName('-accent')).toBe(false);
    expect(isValidOverrideName('--')).toBe(false);
    expect(isValidOverrideName('--Accent')).toBe(false);
    expect(isValidOverrideName('--accent:hover')).toBe(false);
    expect(isValidOverrideName('--a}html{color')).toBe(false);
  });
});

describe('override values', () => {
  it('accepts the shapes real tokens are made of', () => {
    for (const ok of [
      '#58a6ff',
      '1.75rem',
      'rgba(255, 255, 255, 0.72)',
      'linear-gradient(180deg, #15549e 0%, #0b3868 100%)',
      'color-mix(in srgb, var(--accent) 12%, var(--bg-tertiary))',
      '0 0 0.25rem rgba(178, 206, 240, 0.42), 0 0 0.625rem rgba(158, 190, 232, 0.28)',
      'calc(100% - 12rem)',
      '16 / 9',
    ]) {
      expect(isValidOverrideValue(ok), ok).toBe(true);
    }
  });

  it('rejects a value that closes the declaration', () => {
    expect(isValidOverrideValue('red; position: fixed')).toBe(false);
  });

  it('rejects a value that opens a rule or breaks out of a style element', () => {
    expect(isValidOverrideValue('red} html { display: none')).toBe(false);
    expect(isValidOverrideValue('red</style><script>')).toBe(false);
    expect(isValidOverrideValue('@import "http://evil.test/x.css"')).toBe(false);
  });

  it('rejects a value that fetches from another origin', () => {
    expect(isValidOverrideValue('url(http://evil.test/pixel.png)')).toBe(false);
    expect(isValidOverrideValue('URL( http://evil.test/x )')).toBe(false);
    expect(isValidOverrideValue('image-set("http://evil.test/x.png" 1x)')).toBe(false);
    expect(isValidOverrideValue('expression(alert(1))')).toBe(false);
  });

  it('rejects escapes, comments, and oversize or empty values', () => {
    expect(isValidOverrideValue('red\\3b position:fixed')).toBe(false);
    expect(isValidOverrideValue('red /* swallow the rest')).toBe(false);
    expect(isValidOverrideValue('')).toBe(false);
    expect(isValidOverrideValue('   ')).toBe(false);
    expect(isValidOverrideValue('a'.repeat(MAX_STYLE_VALUE_LENGTH + 1))).toBe(false);
    expect(isValidOverrideValue('a'.repeat(MAX_STYLE_VALUE_LENGTH))).toBe(true);
  });
});

describe('parseStyleOverrides', () => {
  it('round-trips a valid map', () => {
    const map = { '--accent': '#ff0000', '--mark-size': '1.8rem' };
    expect(parseStyleOverrides(serializeStyleOverrides(map))).toEqual(map);
  });

  it('drops invalid entries instead of failing the whole map', () => {
    const raw = JSON.stringify({
      '--accent': '#ff0000',
      'color': 'red',
      '--bad': 'red; position: fixed',
      '--n': 12,
    });
    expect(parseStyleOverrides(raw)).toEqual({ '--accent': '#ff0000' });
  });

  it('trims values', () => {
    expect(parseStyleOverrides('{"--accent":"  #fff  "}')).toEqual({ '--accent': '#fff' });
  });

  it('returns an empty map for junk rather than throwing', () => {
    for (const junk of ['', null, undefined, 'not json', '[]', '"str"', '42', 'null']) {
      expect(parseStyleOverrides(junk as string)).toEqual({});
    }
  });

  it('caps the number of entries', () => {
    const big: Record<string, string> = {};
    for (let i = 0; i < MAX_STYLE_OVERRIDES + 50; i++) big[`--t${i}`] = '1px';
    expect(Object.keys(parseStyleOverrides(JSON.stringify(big))).length).toBe(MAX_STYLE_OVERRIDES);
  });
});

describe('styleResetRequested', () => {
  it('matches the escape hatch in its real forms', () => {
    expect(styleResetRequested('?style-reset')).toBe(true);
    expect(styleResetRequested('?a=1&style-reset')).toBe(true);
    expect(styleResetRequested('?style-reset=1')).toBe(true);
    expect(styleResetRequested('?style-reset&b=2')).toBe(true);
  });

  it('does not match a lookalike', () => {
    expect(styleResetRequested('')).toBe(false);
    expect(styleResetRequested('?style-resets=1')).toBe(false);
    expect(styleResetRequested('?not-style-reset')).toBe(false);
  });
});
