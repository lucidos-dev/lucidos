import { describe, it, expect } from 'vitest';
import { scanSources } from '../../components/shared/__tests__/loading-guard-scan';

const THE_WRITER = 'store/thread-events/thread-meta.ts';

const BYPASSES: Array<[string, RegExp]> = [
  ['an assignment to .summaryVersion', /\.summaryVersion\s*=(?!=)/],
  ['an assignment to meta.status', /\bmeta\s*\.\s*status\s*=(?!=)/],
  ['a cast that makes status writable', /as\s*\{[^}]*\bstatus\s*:\s*ThreadStatus/],
  ['an Object.assign onto a meta', /Object\.assign\(\s*[\w.]*meta\b/],
];

describe('thread status has one writer', () => {
  const sources = scanSources('.', ['.ts', '.tsx']).filter(s => s.path !== THE_WRITER);

  it('scans the tree', () => {
    expect(sources.length).toBeGreaterThan(100);
  });

  for (const [what, pattern] of BYPASSES) {
    it(`no source outside ${THE_WRITER} holds ${what}`, () => {
      const offenders = sources.filter(s => pattern.test(s.code)).map(s => s.path);
      expect(offenders).toEqual([]);
    });
  }
});
