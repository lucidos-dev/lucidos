import { describe, expect, it, vi, afterEach } from 'vitest';
import { peerPrefix, peerScopedKey } from './_peerScope';

afterEach(() => vi.unstubAllGlobals());

describe('peer scope', () => {
  it('isolates preview storage from both the own and peer client', () => {
    vi.stubGlobal('document', { querySelector: (selector: string) => selector === 'meta[name="lucidos-frontend-preview"]' ? {} : null });
    expect(peerPrefix(null)).toBe('preview:');
    expect(peerScopedKey('ws:dev:draft', null)).toBe('preview:ws:dev:draft');
    expect(peerScopedKey('ws:dev:draft', 'cloud')).toBe('preview:peer:cloud:ws:dev:draft');
    expect(peerScopedKey('preview:peer:cloud:ws:dev:draft', 'cloud')).toBe('preview:peer:cloud:ws:dev:draft');
  });
  it('prefixes once, and leaves the own client unscoped', () => {
    expect(peerPrefix(null)).toBe('');
    expect(peerScopedKey('ws:dev@cloud:k', null)).toBe('ws:dev@cloud:k');
    expect(peerScopedKey('ws:dev@cloud:k', 'cloud')).toBe('peer:cloud:ws:dev@cloud:k');
    expect(peerScopedKey('peer:cloud:ws:dev@cloud:k', 'cloud')).toBe('peer:cloud:ws:dev@cloud:k');
  });
});
