/**
 * Cold-start bounce decision (shouldBounceToPicker).
 *
 * The reported bug: the PWA cold-start auto-open navigates into the last
 * workspace with no reachability check; when the engine is unreachable the
 * service worker serves the cached shell and the user is stranded. The recovery
 * is to bounce back to the workspace picker — but ONLY on a genuine cold boot,
 * never mid-session, never without a reachable picker, and at most once.
 */
import { describe, it, expect } from 'vitest';
import { shouldBounceToPicker } from './connection';

const stranded = { connectedEver: false, engineAnswered: false, pickerHref: '/~/?pick', alreadyBounced: false };

describe('shouldBounceToPicker', () => {
  it('bounces on a cold boot that never connected, with a reachable picker', () => {
    expect(shouldBounceToPicker(stranded)).toBe(true);
  });

  it('does NOT bounce once we have connected this session (no mid-work yank)', () => {
    expect(shouldBounceToPicker({ ...stranded, connectedEver: true })).toBe(false);
  });

  // An update swapped the engine under a loaded page: its threads came from the
  // old engine, and every health probe after that failed until the new one was
  // up. That is a restart, not a stranded shell.
  it('does NOT bounce a page its engine already served, whatever health says', () => {
    expect(shouldBounceToPicker({ ...stranded, engineAnswered: true })).toBe(false);
  });

  it('does NOT bounce when there is no picker (legacy direct engine, href null)', () => {
    expect(shouldBounceToPicker({ ...stranded, pickerHref: null })).toBe(false);
  });

  it('does NOT bounce again once it has bounced (one-shot, loop-safe)', () => {
    expect(shouldBounceToPicker({ ...stranded, alreadyBounced: true })).toBe(false);
  });
});
