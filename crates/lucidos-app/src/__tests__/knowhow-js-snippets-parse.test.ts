import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../..');

const DOC = 'system-knowhow/deriving-an-api-from-a-site.md';
const EXPECTED_SNIPPETS = 2;

function jsSnippets(relPath: string): string[] {
  const md = readFileSync(resolve(REPO_ROOT, relPath), 'utf-8');
  return [...md.matchAll(/```js\n([\s\S]*?)```/g)].map((m) => m[1]);
}

describe('knowhow JavaScript snippets', () => {
  const snippets = jsSnippets(DOC);

  it('finds every snippet the doc is meant to ship', () => {
    expect(snippets).toHaveLength(EXPECTED_SNIPPETS);
  });

  it.each(snippets.map((s, i) => [i, s]))('snippet %i parses', (_i, src) => {
    expect(() => new Function(src)).not.toThrow();
  });

  it('never hardcodes a secret to paste', () => {
    for (const src of snippets) {
      expect(src).not.toMatch(/eyJ[A-Za-z0-9_-]{10,}/);
    }
  });
});
