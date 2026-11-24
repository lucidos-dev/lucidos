import { describe, it, expect } from 'vitest';
import orderedVersions from '../generated/version-order-fixture.json';
import { isNewerBuildId, isNewerVersion, isValidReleaseVersion } from './version';

describe('isNewerVersion', () => {
  it('agrees with Rust for every pair in the generated semver fixture', () => {
    for (const [i, current] of orderedVersions.entries()) {
      expect(isValidReleaseVersion(current)).toBe(true);
      for (const [j, offered] of orderedVersions.entries()) {
        expect(isNewerVersion(offered, current), `${current} -> ${offered}`).toBe(j > i);
      }
    }
  });

  it('ignores build metadata and trims outer whitespace', () => {
    expect(isNewerVersion('1.0.0+z', '1.0.0+a')).toBe(false);
    expect(isNewerVersion('1.0.0+a', '1.0.0+z')).toBe(false);
    expect(isNewerVersion(' 1.0.1 ', '\t1.0.0\n')).toBe(true);
    expect(isNewerVersion('1.0.0-rc.1+build', '1.0.0-rc.1')).toBe(false);
  });

  it.each([
    '', 'unknown', 'v1.0.0', '1.0', '2026.03.23.11', '01.0.0',
    '1.0.0-01', '1.0.0-a..b', '1.0.0+', '1.0.0-a_b', '1.0.0+é',
    '1.0.0-rc.1\nextra', '18446744073709551616.0.0',
  ])('fails closed for invalid version %j in either position', invalid => {
    expect(isValidReleaseVersion(invalid)).toBe(false);
    expect(isNewerVersion(invalid, '1.0.0')).toBe(false);
    expect(isNewerVersion('1.0.0', invalid)).toBe(false);
  });

  it('compares numeric identifiers without losing precision', () => {
    expect(isNewerVersion('9007199254740993.0.0', '9007199254740992.0.0')).toBe(true);
    expect(isNewerVersion('1.0.0-9007199254740993', '1.0.0-9007199254740992')).toBe(true);
    expect(isNewerVersion('1.0.0-a', '1.0.0-999999999999999999999999')).toBe(true);
  });
});

describe('isNewerBuildId', () => {
  it('keeps dev build update ordering across patch and date changes', () => {
    expect(isNewerBuildId('2026.03.23.11', '2026.03.23.8')).toBe(true);
    expect(isNewerBuildId('2026.03.24.1', '2026.03.23.11')).toBe(true);
    expect(isNewerBuildId('2026.03.23.8', '2026.03.23.11')).toBe(false);
    expect(isNewerBuildId('2026.03.23.8', '2026.03.23.8')).toBe(false);
  });

  it.each(['unknown', '', '1.2.3', '2026.03.23.x'])('refuses invalid build ID %j', id => {
    expect(isNewerBuildId(id, '2026.03.23.8')).toBe(false);
    expect(isNewerBuildId('2026.03.23.8', id)).toBe(false);
  });
});
