import { describe, it, expect } from 'vitest';
import { collectSearchResults, filterSearchResults, type FileSearchResult } from '../fileSearch';

describe('collectSearchResults', () => {
  it('collects workspace files', () => {
    const results = collectSearchResults(
      ['data/intents/daily.md', 'data/knowhow/git.md'],
      [], [], [],
    );
    expect(results).toEqual([
      { path: 'data/intents/daily.md', source: 'workspace' },
      { path: 'data/knowhow/git.md', source: 'workspace' },
    ]);
  });

  it('collects repo files', () => {
    const results = collectSearchResults(
      [],
      ['src/main.rs', 'src/lib.rs'],
      [], [],
    );
    expect(results).toEqual([
      { path: 'src/main.rs', source: 'repo' },
      { path: 'src/lib.rs', source: 'repo' },
    ]);
  });

  it('collects diff change files with status', () => {
    const results = collectSearchResults(
      [], [],
      [{ path: 'src/api.rs', status: 'modified' }, { path: 'src/new.rs', status: 'added' }],
      [],
    );
    expect(results).toEqual([
      { path: 'src/api.rs', source: 'change', changeStatus: 'modified' },
      { path: 'src/new.rs', source: 'change', changeStatus: 'added' },
    ]);
  });

  it('collects CC change files without status', () => {
    const results = collectSearchResults(
      [], [], [],
      [{ path: 'crates/app/src/App.tsx' }],
    );
    expect(results).toEqual([
      { path: 'crates/app/src/App.tsx', source: 'change', changeStatus: undefined },
    ]);
  });

  it('combines all sources', () => {
    const results = collectSearchResults(
      ['data/readme.md'],
      ['src/main.rs'],
      [{ path: 'src/api.rs', status: 'modified' }],
      [{ path: 'crates/app/index.ts' }],
    );
    expect(results).toHaveLength(4);
    expect(results[0]).toEqual({ path: 'data/readme.md', source: 'workspace' });
    expect(results[1]).toEqual({ path: 'src/main.rs', source: 'repo' });
    expect(results[2]).toEqual({ path: 'src/api.rs', source: 'change', changeStatus: 'modified' });
    expect(results[3]).toEqual({ path: 'crates/app/index.ts', source: 'change', changeStatus: undefined });
  });

  it('deduplicates diff and CC change files by path', () => {
    const results = collectSearchResults(
      [], [],
      [{ path: 'src/api.rs', status: 'modified' }],
      [{ path: 'src/api.rs' }],
    );
    expect(results).toHaveLength(1);
    expect(results[0].source).toBe('change');
  });

  it('allows same path in different sources (workspace and repo)', () => {
    const results = collectSearchResults(
      ['README.md'],
      ['README.md'],
      [], [],
    );
    expect(results).toHaveLength(2);
    expect(results[0]).toEqual({ path: 'README.md', source: 'workspace' });
    expect(results[1]).toEqual({ path: 'README.md', source: 'repo' });
  });

  it('returns empty array for no inputs', () => {
    const results = collectSearchResults([], [], [], []);
    expect(results).toEqual([]);
  });
});

describe('filterSearchResults', () => {
  const allResults: FileSearchResult[] = [
    { path: 'data/intents/daily.md', source: 'workspace' },
    { path: 'src/main.rs', source: 'repo' },
    { path: 'src/api.rs', source: 'change', changeStatus: 'modified' },
    { path: 'crates/app/FileSearch.tsx', source: 'change', changeStatus: 'added' },
  ];

  it('returns all results for empty query', () => {
    expect(filterSearchResults(allResults, '')).toEqual(allResults);
  });

  it('returns all results for whitespace query', () => {
    expect(filterSearchResults(allResults, '   ')).toEqual(allResults);
  });

  it('filters by substring match on path', () => {
    const filtered = filterSearchResults(allResults, 'api');
    expect(filtered).toHaveLength(1);
    expect(filtered[0].path).toBe('src/api.rs');
  });

  it('is case-insensitive', () => {
    const filtered = filterSearchResults(allResults, 'FILESEARCH');
    expect(filtered).toHaveLength(1);
    expect(filtered[0].path).toBe('crates/app/FileSearch.tsx');
  });

  it('matches across path segments', () => {
    const filtered = filterSearchResults(allResults, 'intents/daily');
    expect(filtered).toHaveLength(1);
    expect(filtered[0].path).toBe('data/intents/daily.md');
  });

  it('returns empty for no matches', () => {
    const filtered = filterSearchResults(allResults, 'nonexistent');
    expect(filtered).toHaveLength(0);
  });

  it('filters across all sources', () => {
    const filtered = filterSearchResults(allResults, '.rs');
    expect(filtered).toHaveLength(2);
    expect(filtered.map(r => r.source)).toEqual(['repo', 'change']);
  });
});
