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
