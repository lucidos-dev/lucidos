import { describe, it, expect } from 'vitest';
import postcss from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { resolve } from 'node:path';
import { ENGINE_SERVED_CSS, REPO_ROOT } from './css-rule-helpers';

describe('engine-served stylesheets', () => {
  for (const relPath of ENGINE_SERVED_CSS) {
    it(`parses: ${relPath}`, () => {
      const path = resolve(REPO_ROOT, relPath);
      const css = readFileSync(path, 'utf8');
      let error: string | null = null;
      try {
        postcss.parse(css, { from: path });
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
      expect(
        error,
        `${relPath} is served to every app iframe but is parsed by no build step, `
        + 'so a syntax error here ships silently. Fix the CSS.',
      ).toBeNull();
    });
  }
});
