import { describe, it, expect } from 'vitest';
import { basename } from './FilePreviewInline';

describe('basename', () => {
  it('returns the last segment of a nested path', () => {
    expect(basename('apps/no-role-playing-0.1.2.lucidos-plugin'))
      .toBe('no-role-playing-0.1.2.lucidos-plugin');
  });

  it('returns the path itself when it has no slashes', () => {
    expect(basename('foo.bin')).toBe('foo.bin');
  });

  it('handles deeply nested paths', () => {
    expect(basename('a/b/c/file.tar.gz')).toBe('file.tar.gz');
  });

  it('returns empty string for empty input', () => {
    expect(basename('')).toBe('');
  });

  it('returns empty string for trailing-slash paths', () => {
    expect(basename('apps/')).toBe('');
  });
});
