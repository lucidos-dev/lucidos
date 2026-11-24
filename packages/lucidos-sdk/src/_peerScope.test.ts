import { describe, expect, it } from 'vitest';
import { peerPrefix, peerScopedKey } from './_peerScope';

describe('peer scope', () => {
  it('prefixes once, and leaves the own client unscoped', () => {
    expect(peerPrefix(null)).toBe('');
    expect(peerScopedKey('ws:dev@cloud:k', null)).toBe('ws:dev@cloud:k');
    expect(peerScopedKey('ws:dev@cloud:k', 'cloud')).toBe('peer:cloud:ws:dev@cloud:k');
    expect(peerScopedKey('peer:cloud:ws:dev@cloud:k', 'cloud')).toBe('peer:cloud:ws:dev@cloud:k');
  });
});
