import { describe, it, expect } from 'vitest';
import { isUnstampedBuildId } from './buildId';

describe('isUnstampedBuildId', () => {
  it('recognises the placeholder the stamp plugin leaves in the live dev server', () => {
    expect(isUnstampedBuildId('__LUCIDOS_BUILD_ID__')).toBe(true);
  });

  it('treats a real stamped id as carrying a staleness signal', () => {
    expect(isUnstampedBuildId('1ba1c823d933')).toBe(false);
  });

  it('does not match an id that merely contains underscores', () => {
    expect(isUnstampedBuildId('a__b')).toBe(false);
  });
});
