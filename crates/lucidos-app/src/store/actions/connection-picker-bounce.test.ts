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

describe('shouldBounceToPicker', () => {
  it('bounces on a cold boot that never connected, with a reachable picker', () => {
    expect(shouldBounceToPicker({ connectedEver: false, pickerHref: '/~/?pick', alreadyBounced: false })).toBe(true);
  });

  it('does NOT bounce once we have connected this session (no mid-work yank)', () => {
    expect(shouldBounceToPicker({ connectedEver: true, pickerHref: '/~/?pick', alreadyBounced: false })).toBe(false);
  });

  it('does NOT bounce when there is no picker (legacy direct engine, href null)', () => {
    expect(shouldBounceToPicker({ connectedEver: false, pickerHref: null, alreadyBounced: false })).toBe(false);
  });

  it('does NOT bounce again once it has bounced (one-shot, loop-safe)', () => {
    expect(shouldBounceToPicker({ connectedEver: false, pickerHref: '/~/?pick', alreadyBounced: true })).toBe(false);
  });
});
