import { describe, it, expect } from 'vitest';
import {
  parseBindValue,
  toBindValue,
  isValidIp,
  isValidBindSelection,
} from './bindMode';

describe('parseBindValue', () => {
  it('maps keywords (case-insensitive) and blank to modes', () => {
    expect(parseBindValue('loopback')).toEqual({ mode: 'loopback', address: '' });
    expect(parseBindValue('LOOPBACK')).toEqual({ mode: 'loopback', address: '' });
    expect(parseBindValue('')).toEqual({ mode: 'loopback', address: '' });
    expect(parseBindValue('all')).toEqual({ mode: 'all', address: '' });
  });

  it('maps an IP literal to address mode, preserving the original text', () => {
    expect(parseBindValue('100.101.71.58')).toEqual({
      mode: 'address',
      address: '100.101.71.58',
    });
  });
});

describe('toBindValue', () => {
  it('round-trips with parseBindValue', () => {
    expect(toBindValue('loopback', '')).toBe('loopback');
    expect(toBindValue('all', '')).toBe('all');
    expect(toBindValue('address', ' 100.101.71.58 ')).toBe('100.101.71.58');
  });
});

describe('isValidIp', () => {
  it('accepts valid IPv4', () => {
    expect(isValidIp('100.101.71.58')).toBe(true);
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

describe('isValidBindSelection', () => {
  it('keyword modes are always valid; address mode needs a valid IP', () => {
    expect(isValidBindSelection('loopback', '')).toBe(true);
    expect(isValidBindSelection('all', '')).toBe(true);
    expect(isValidBindSelection('address', '100.101.71.58')).toBe(true);
    expect(isValidBindSelection('address', 'garbage')).toBe(false);
    expect(isValidBindSelection('address', '')).toBe(false);
  });
});
