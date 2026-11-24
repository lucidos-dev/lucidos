import { describe, it, expect, vi } from 'vitest';
import { provideReleaseCheck, refreshReleaseCheckIfShown } from './releaseCheckHandle';

describe('release check handle', () => {
  it('skips the re-read before the shell provides it', () => {
    expect(() => refreshReleaseCheckIfShown()).not.toThrow();
  });

  it('forwards to the provided refresh, with no force', () => {
    const refresh = vi.fn().mockResolvedValue('current');
    provideReleaseCheck(refresh);
    refreshReleaseCheckIfShown();
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(refresh).toHaveBeenCalledWith();
  });
});
