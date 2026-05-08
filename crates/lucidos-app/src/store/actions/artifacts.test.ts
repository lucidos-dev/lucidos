import { describe, it, expect } from 'vitest';
import { normalizeDataPath } from './artifacts';

describe('normalizeDataPath', () => {
  it('prepends artifacts/ when path has no known prefix', () => {
    expect(normalizeDataPath('research/lucidos-product-market-fit.md')).toBe(
      'artifacts/research/lucidos-product-market-fit.md',
    );
  });

  it('leaves artifacts/ paths unchanged', () => {
    expect(normalizeDataPath('artifacts/research/notes.md')).toBe(
      'artifacts/research/notes.md',
    );
  });

  it('leaves knowhow/ paths unchanged', () => {
    expect(normalizeDataPath('knowhow/domain/guide.md')).toBe(
      'knowhow/domain/guide.md',
    );
  });

  it('leaves apps/ paths unchanged', () => {
    expect(normalizeDataPath('apps/myapp/knowhow/file.md')).toBe(
      'apps/myapp/knowhow/file.md',
    );
  });

  it('leaves triggers/ paths unchanged', () => {
    expect(normalizeDataPath('triggers/daily/check.md')).toBe(
      'triggers/daily/check.md',
    );
  });

  it('leaves system-knowhow/ paths unchanged', () => {
    expect(normalizeDataPath('system-knowhow/best-practices.md')).toBe(
      'system-knowhow/best-practices.md',
    );
  });

  it('handles bare filename without directory', () => {
    expect(normalizeDataPath('readme.md')).toBe('artifacts/readme.md');
  });
});
