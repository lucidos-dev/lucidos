import { describe, expect, it } from 'vitest';
import { createScrollOwner } from '../scrollOwner';

describe('transcript position ownership', () => {
  it('gives a ready older link authority without releasing the newer claim', () => {
    const position = createScrollOwner();
    const older = {};
    const newer = {};

    position.transition({ type: 'arm' });
    position.transition({ type: 'claim-link', claim: older });
    position.transition({ type: 'claim-link', claim: newer });
    const olderWriter = position.landLink(older, false);
    position.transition({ type: 'link-landed', claim: older, atEdge: false });

    expect(position.owner()).toEqual({ kind: 'link', claim: older });
    expect(position.claim()).toBe(newer);
    expect(position.armed()).toBe(false);
    expect(position.claimHasLanded()).toBe(false);
    expect(position.linkLandedOffEdge()).toBe(true);

    position.transition({ type: 'finish', owner: olderWriter });
    position.transition({ type: 'release-link', claim: older });
    expect(position.claim()).toBe(newer);

    position.landLink(newer, false);
    position.transition({ type: 'link-landed', claim: newer, atEdge: false });
    expect(position.owner()).toEqual({ kind: 'link', claim: newer });
    expect(position.claimHasLanded()).toBe(true);
  });

  it.each(['landing', 'navigation', 'hold', 'restore'] as const)(
    'returns the position to the ride after a %s write',
    (kind) => {
      const position = createScrollOwner();
      position.transition({ type: 'arm' });
      position.withWriter({ kind }, () => {
        expect(position.owner()).toEqual({ kind });
        expect(position.armed()).toBe(true);
      });
      expect(position.owner()).toEqual({ kind: 'ride' });
    },
  );

  it('parks on a waiting thread and rides when the reader returns to the edge', () => {
    const position = createScrollOwner();
    position.transition({ type: 'arm' });
    const gesture = { type: 'scroll', tookOver: true, gesture: true, placement: false, live: false } as const;
    position.transition({ ...gesture, atEdge: false });
    expect(position.owner()).toEqual({ kind: 'reader' });
    expect(position.armed()).toBe(true);
    position.transition({ ...gesture, atEdge: true });
    expect(position.owner()).toEqual({ kind: 'ride' });
  });

  it('keeps restore attempts separate and lends the position only for their writes', () => {
    const position = createScrollOwner();
    const first = position.beginRestore();
    const second = position.beginRestore();
    first.finish();
    expect(first.waiting()).toBe(false);
    expect(second.waiting()).toBe(true);
    second.write(() => expect(position.owner()).toEqual({ kind: 'restore' }));
    expect(position.owner()).toEqual({ kind: 'reader' });
  });

  it('leaves the ride on a confirmed off-edge placement', () => {
    const position = createScrollOwner();
    position.transition({ type: 'arm' });
    position.transition({ type: 'scroll', atEdge: false, tookOver: true, gesture: false, placement: true, live: true });
    expect(position.owner()).toEqual({ kind: 'reader' });
    expect(position.armed()).toBe(false);
  });

  it('keeps the ride on a platform scroll at the same offset', () => {
    const position = createScrollOwner();
    position.transition({ type: 'arm' });
    position.transition({ type: 'scroll', atEdge: false, tookOver: true, gesture: false, placement: false, live: true });
    expect(position.owner()).toEqual({ kind: 'ride' });
    expect(position.armed()).toBe(true);
  });
});
