import { describe, it, expect } from 'vitest';
import { errorDetail } from './errorDetail';

describe('errorDetail', () => {
  it('maps DOMException AbortError to "request timed out"', () => {
    const err = new DOMException('aborted', 'AbortError');
    expect(errorDetail(err)).toBe('request timed out');
  });

  it('returns Error.message for plain errors', () => {
    expect(errorDetail(new Error('boom'))).toBe('boom');
  });

  it('coerces unknown values via String()', () => {
    expect(errorDetail('weird string')).toBe('weird string');
    expect(errorDetail(42)).toBe('42');
    expect(errorDetail(null)).toBe('null');
  });
});
