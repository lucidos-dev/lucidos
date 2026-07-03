import { describe, it, expect } from 'vitest';
import { classifyStartup } from './liveness';

describe('classifyStartup', () => {
  const base = {
    deadMs: null as number | null,
    cleanClose: false,
    navType: 'navigate',
    isIOSPwa: true,
  };

  it('returns cold when there is no prior heartbeat', () => {
    expect(classifyStartup({ ...base, deadMs: null })).toBe('cold');
  });

  it('returns cold when the dead time exceeds 30 minutes', () => {
    expect(classifyStartup({ ...base, deadMs: 31 * 60 * 1000 })).toBe('cold');
  });

  it('returns reload_clean when pagehide fired and nav type is reload', () => {
    expect(
      classifyStartup({ ...base, deadMs: 1000, cleanClose: true, navType: 'reload' }),
    ).toBe('reload_clean');
  });

  it('returns nav_clean when pagehide fired and nav type is not reload', () => {
    expect(
      classifyStartup({ ...base, deadMs: 5000, cleanClose: true, navType: 'navigate' }),
    ).toBe('nav_clean');
  });

  it('returns likely_crash on iOS PWA with a short dead window and no clean close', () => {
    expect(
      classifyStartup({ ...base, deadMs: 3000, cleanClose: false, isIOSPwa: true }),
    ).toBe('likely_crash');
  });

  it('does not flag non-iOS-PWA short dead windows as crash', () => {
    expect(
      classifyStartup({ ...base, deadMs: 3000, cleanClose: false, isIOSPwa: false }),
    ).toBe('bg_resume');
  });

  it('returns bg_resume for longer iOS-PWA dead windows without clean close', () => {
    // 60s is past the crash-gap threshold but well under the cold threshold.
    expect(
      classifyStartup({ ...base, deadMs: 60_000, cleanClose: false, isIOSPwa: true }),
    ).toBe('bg_resume');
  });

  it('treats the crash threshold boundary as crash on iOS PWA', () => {
    expect(
      classifyStartup({ ...base, deadMs: 7_000, cleanClose: false, isIOSPwa: true }),
    ).toBe('likely_crash');
  });

  it('treats one millisecond past the crash threshold as bg_resume', () => {
    expect(
      classifyStartup({ ...base, deadMs: 7_001, cleanClose: false, isIOSPwa: true }),
    ).toBe('bg_resume');
  });
});
