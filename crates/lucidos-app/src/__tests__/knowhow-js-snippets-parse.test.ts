/**
 * `system-knowhow/deriving-an-api-from-a-site.md` ships executable JavaScript
 * that an agent pastes verbatim into a live page via `browser_eval`. Nothing
 * else parses it: it is fenced markdown, so `tsc` ignores it, it is outside
 * the Vite graph, and the Rust suite treats the file as opaque bytes.
 *
 * A syntax error therefore ships clean and only surfaces mid-task, as a
 * `browser_eval` that throws while the user waits with a site open. The agent
 * cannot tell a broken snippet from a hostile page, so it burns turns retrying.
 *
 * Same shape as `styles/__tests__/engine-served-css-parses.test.ts`, for the
 * same reason: a shipped asset no build step reads needs its own gate.
 *
 * This checks syntax, not behavior. The snippets were verified against a real
 * Chromium when written, covering capture, redaction and the reload limit.
 * Add a snippet here in the same change that adds it to the doc.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
/** Repo root, from `crates/lucidos-app/src/__tests__/`. */
const REPO_ROOT = resolve(here, '../../../..');

const DOC = 'system-knowhow/deriving-an-api-from-a-site.md';
/** Every ```js fence in the doc, in document order. */
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
    // `new Function` compiles the body without running it, which is the point:
    // these snippets touch `window` and would throw under Vitest. Passing the
    // source as a bare body accepts both an expression and statements, so a
    // future statement-shaped snippet does not fail for the wrong reason.
    expect(() => new Function(src)).not.toThrow();
  });

  it('never hardcodes a secret to paste', () => {
    for (const src of snippets) {
      expect(src).not.toMatch(/eyJ[A-Za-z0-9_-]{10,}/);
    }
  });
});
