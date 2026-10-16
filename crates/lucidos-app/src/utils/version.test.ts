import { describe, it, expect } from 'vitest';
import { isNewerVersion } from './version';

describe('isNewerVersion', () => {
  it('returns true when a > b (higher patch)', () => {
    expect(isNewerVersion('2026.03.23.11', '2026.03.23.8')).toBe(true);
  });

  it('returns false when a < b (lower patch)', () => {
    expect(isNewerVersion('2026.03.23.8', '2026.03.23.11')).toBe(false);
  });

  it('returns false when equal', () => {
    expect(isNewerVersion('2026.03.23.8', '2026.03.23.8')).toBe(false);
  });

  it('returns true when a has higher day', () => {
    expect(isNewerVersion('2026.03.24.1', '2026.03.23.11')).toBe(true);
  });

  it('handles different segment counts', () => {
    expect(isNewerVersion('2026.03.23.1', '2026.03.23')).toBe(true);
    expect(isNewerVersion('2026.03.23', '2026.03.23.1')).toBe(false);
  });

  it('treats "unknown" as not newer in either position', () => {
    // read_engine_version returns "unknown" when VERSION file is missing.
    // NaN from "unknown".split('.').map(Number) makes all comparisons false,
    // so neither side is considered "newer" — no false update triggers.
    expect(isNewerVersion('unknown', '2026.04.13.1')).toBe(false);
    expect(isNewerVersion('2026.04.13.1', 'unknown')).toBe(false);
  });
});
