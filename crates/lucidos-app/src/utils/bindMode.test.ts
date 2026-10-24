import { describe, it, expect } from 'vitest';
import {
  parseBindValue,
  toBindValue,
  isValidIp,
  isValidBindSelection,
  draftFromBind,
  bindDraftMatchesSaved,
} from './bindMode';

describe('parseBindValue', () => {
  it('maps keywords (case-insensitive) and blank to modes', () => {
    expect(parseBindValue('loopback')).toEqual({ mode: 'loopback', address: '' });
    expect(parseBindValue('LOOPBACK')).toEqual({ mode: 'loopback', address: '' });
    expect(parseBindValue('')).toEqual({ mode: 'loopback', address: '' });
    expect(parseBindValue('all')).toEqual({ mode: 'all', address: '' });
  });

  it('maps an IP literal to address mode, preserving the original text', () => {
    expect(parseBindValue('100.64.0.1')).toEqual({
      mode: 'address',
      address: '100.64.0.1',
    });
  });
});

describe('toBindValue', () => {
  it('round-trips with parseBindValue', () => {
    expect(toBindValue('loopback', '')).toBe('loopback');
    expect(toBindValue('all', '')).toBe('all');
    expect(toBindValue('address', ' 100.64.0.1 ')).toBe('100.64.0.1');
  });
});

describe('isValidIp', () => {
  it('accepts valid IPv4', () => {
    expect(isValidIp('100.64.0.1')).toBe(true);
    expect(isValidIp('127.0.0.1')).toBe(true);
    expect(isValidIp('0.0.0.0')).toBe(true);
  });

  it('rejects malformed IPv4 (out of range, leading zeros, wrong arity)', () => {
    expect(isValidIp('256.1.1.1')).toBe(false);
    expect(isValidIp('100.01.1.1')).toBe(false);
    expect(isValidIp('1.2.3')).toBe(false);
    expect(isValidIp('1.2.3.4.5')).toBe(false);
    expect(isValidIp('hello')).toBe(false);
    expect(isValidIp('')).toBe(false);
  });

  it('accepts basic IPv6 and rejects junk', () => {
    expect(isValidIp('::1')).toBe(true);
    expect(isValidIp('fd7a:115c:a1e0::1')).toBe(true);
    expect(isValidIp('not:valid:ipv6:xyz!')).toBe(false);
    expect(isValidIp('1::2::3')).toBe(false);
  });
});

describe('draftFromBind', () => {
  it('seeds the draft from the saved value, so it opens on what is stored', () => {
    expect(draftFromBind('all', true)).toEqual({ mode: 'all', address: '', inherit: true });
    expect(draftFromBind('loopback', false)).toEqual({
      mode: 'loopback',
      address: '',
      inherit: false,
    });
    expect(draftFromBind('100.64.0.1', true)).toEqual({
      mode: 'address',
      address: '100.64.0.1',
      inherit: true,
    });
  });

  it('round-trips through toBindValue, so an untouched draft saves what it loaded', () => {
    for (const bind of ['loopback', 'all', '100.64.0.1']) {
      const d = draftFromBind(bind, true);
      expect(toBindValue(d.mode, d.address)).toBe(bind);
    }
  });
});

describe('bindDraftMatchesSaved', () => {
  it('a freshly seeded draft matches, so Save has nothing to offer on open', () => {
    for (const bind of ['loopback', 'all', '100.64.0.1']) {
      expect(bindDraftMatchesSaved(draftFromBind(bind, true), bind, true)).toBe(true);
    }
  });

  it('detects a changed mode, address, or inherit flag', () => {
    expect(bindDraftMatchesSaved(draftFromBind('loopback', true), 'all', true)).toBe(false);
    expect(bindDraftMatchesSaved(draftFromBind('100.64.0.2', true), '100.64.0.1', true)).toBe(
      false,
    );
    expect(bindDraftMatchesSaved(draftFromBind('all', false), 'all', true)).toBe(false);
  });

  it('normalizes both sides, so case and whitespace alone are not a change', () => {
    expect(bindDraftMatchesSaved({ mode: 'all', address: '', inherit: true }, 'ALL', true)).toBe(
      true,
    );
    expect(
      bindDraftMatchesSaved(
        { mode: 'address', address: ' 100.64.0.1 ', inherit: true },
        '100.64.0.1',
        true,
      ),
    ).toBe(true);
  });

  it('ignores address text in the modes that do not use it', () => {
    // Type an IP, then pick "all": the write would still be "all", so there is
    // nothing to save and the button must stay disabled.
    expect(
      bindDraftMatchesSaved({ mode: 'all', address: '100.64.0.1', inherit: true }, 'all', true),
    ).toBe(true);
  });
});

describe('isValidBindSelection', () => {
  it('keyword modes are always valid; address mode needs a valid IP', () => {
    expect(isValidBindSelection('loopback', '')).toBe(true);
    expect(isValidBindSelection('all', '')).toBe(true);
    expect(isValidBindSelection('address', '100.64.0.1')).toBe(true);
    expect(isValidBindSelection('address', 'garbage')).toBe(false);
    expect(isValidBindSelection('address', '')).toBe(false);
  });
});
