import { describe, it, expect } from 'vitest';
import { errorDetail } from './errorDetail';

describe('errorDetail', () => {
  it('maps DOMException TimeoutError to "request timed out"', () => {
    // AbortSignal.timeout(ms) firing surfaces as a TimeoutError DOMException.
    const err = new DOMException('timed out', 'TimeoutError');
    expect(errorDetail(err)).toBe('request timed out');
  });

  it('maps DOMException AbortError to "request cancelled"', () => {
    // controller.abort() (no args) surfaces as an AbortError DOMException —
    // user cancel, not a timeout.
    const err = new DOMException('aborted', 'AbortError');
    expect(errorDetail(err)).toBe('request cancelled');
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
