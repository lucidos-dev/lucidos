import { expect, it } from 'vitest';

// Import ownership prevents aliased parser or sanitizer bypasses too.
const sources = import.meta.glob('../**/*.{ts,tsx}', { query: '?raw', import: 'default', eager: true });
const owners: Record<string, string> = {
  './markdownEngine.ts': 'owns sanitization, resolution and remote scoping',
  './markdownLibraries.ts': 'loads the parser and sanitizer behind the rendering gate',
  './markedConfig.ts': 'configures the shared parser without exposing HTML',
  '../components/files/RenderedDiff.tsx': 'token parser finalizes its joined document through sanitizeHtmlFragments',
};

it('keeps parser and sanitizer ownership closed to new production entry points', () => {
  const found: string[] = [];
  for (const [path, raw] of Object.entries(sources)) {
    if (/\.(test|guard\.test)\.|\/__tests__\//.test(path)) continue;
    if (/from\s*['"](?:marked|dompurify)['"]/.test(String(raw))) found.push(path);
  }
  expect(found.sort()).toEqual(Object.keys(owners).sort());
  expect(String(sources['../components/files/RenderedDiff.tsx'])).toContain(
    "return sanitizeHtmlFragments(unwrapPathEmailAutolinks(parts.join('')), doc)",
  );
});
