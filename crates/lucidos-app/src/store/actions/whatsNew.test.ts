import { describe, it, expect, beforeEach } from 'vitest';
import { hasUnreadWhatsNew, markWhatsNewSeen } from './whatsNew';
import { whatsNewSeenRelease } from '../store';

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
