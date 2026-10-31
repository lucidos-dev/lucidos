import { describe, it, expect, beforeEach, vi } from 'vitest';
import { hasUnreadWhatsNew, markWhatsNewSeen, reloadChangelogIfRead } from './whatsNew';
import { changelogReleases, whatsNewSeenRelease } from '../store';
import { engineChangelog } from '../../api/client';

vi.mock('../../api/client', () => ({
  engineChangelog: vi.fn(),
}));

/**
 * The reported defect: What's New opened while an update restart had the
 * engine down, so the read failed. Nothing asked again, and the panel showed
 * "Failed to load the changelog" until the app was restarted by hand.
 */
describe('reloadChangelogIfRead', () => {
  const release = { version: '1.0.0', date: null, notes: 'body' };

  beforeEach(() => {
    vi.mocked(engineChangelog).mockReset();
  });

  it('recovers a read that failed while the engine was down', async () => {
    changelogReleases.value = { status: 'failed', error: 'Load failed' };
    vi.mocked(engineChangelog).mockResolvedValue([release]);

    await reloadChangelogIfRead();

    expect(changelogReleases.value).toEqual({ status: 'loaded', data: [release] });
  });

  it('asks the new engine again, since the changelog follows its release', async () => {
    changelogReleases.value = { status: 'loaded', data: [] };
    vi.mocked(engineChangelog).mockResolvedValue([release]);

    await reloadChangelogIfRead();

    expect(changelogReleases.value).toEqual({ status: 'loaded', data: [release] });
  });

  it('keeps a loaded list when the reload fails', async () => {
    changelogReleases.value = { status: 'loaded', data: [release] };
    vi.mocked(engineChangelog).mockRejectedValue(new Error('Load failed'));

    await reloadChangelogIfRead();

    expect(changelogReleases.value).toEqual({ status: 'loaded', data: [release] });
  });

  it('fetches nothing for a client that never opened the panel', async () => {
    changelogReleases.value = { status: 'not-loaded' };

    await reloadChangelogIfRead();

    expect(engineChangelog).not.toHaveBeenCalled();
    expect(changelogReleases.value).toEqual({ status: 'not-loaded' });
  });
});

describe('hasUnreadWhatsNew', () => {
  it('shows the dot for a client that has never opened the panel', () => {
    // Including a brand new install: one dot pointing at what is in the
    // version they just installed, cleared by opening it.
    expect(hasUnreadWhatsNew('0.26.3', null)).toBe(true);
  });

  it('shows the dot once the running release moves past the one that was read', () => {
    expect(hasUnreadWhatsNew('0.26.3', '0.26.2')).toBe(true);
  });

  it('stays quiet once this release has been read', () => {
    expect(hasUnreadWhatsNew('0.26.3', '0.26.3')).toBe(false);
  });

  it('stays quiet while the running release is still unknown', () => {
    // The window before /health answers. Treating it as new would flash a dot
    // on every single reload.
    expect(hasUnreadWhatsNew(null, null)).toBe(false);
    expect(hasUnreadWhatsNew(null, '0.26.3')).toBe(false);
  });
});

describe('markWhatsNewSeen', () => {
  beforeEach(() => {
    localStorage.clear();
    whatsNewSeenRelease.value = null;
  });

  it('records the release, so the dot clears and stays cleared', () => {
    markWhatsNewSeen('0.26.3');
    expect(whatsNewSeenRelease.value).toBe('0.26.3');
    expect(localStorage.getItem('lucidos-whats-new-seen-release')).toBe('0.26.3');
    expect(hasUnreadWhatsNew('0.26.3', whatsNewSeenRelease.value)).toBe(false);
  });

  it('never records an unknown release', () => {
    // Opening the panel before /health answers must not spend the one
    // notification the user gets for whatever release they are actually on.
    markWhatsNewSeen(null);
    expect(whatsNewSeenRelease.value).toBe(null);
    expect(localStorage.getItem('lucidos-whats-new-seen-release')).toBe(null);
  });
});
