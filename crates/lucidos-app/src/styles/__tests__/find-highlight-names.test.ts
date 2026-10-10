/**
 * The find bar's two highlight names are written once, in the matcher
 * (`packages/lucidos-sdk/src/find.ts`). CSS cannot import them, so this pins
 * the two stylesheets that style them: the host's own, and the one served to
 * app frames. A rename on one side would paint nothing and say nothing.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { ALL_HIGHLIGHT, CURRENT_HIGHLIGHT } from '@lucidos/find';

const here: string = dirname(fileURLToPath(import.meta.url));
const SHEETS = {
  host: resolve(here, '../global/host-components.css'),
  'app frames': resolve(here, '../../../../lucidos-engine/src/api/sdk_iframe.css'),
};

describe('the find highlights are styled under the names the matcher paints', () => {
  for (const [who, path] of Object.entries(SHEETS)) {
    it(`for ${who}`, () => {
      const css: string = readFileSync(path, 'utf-8');
      expect(css).toContain(`::highlight(${ALL_HIGHLIGHT})`);
      expect(css).toContain(`::highlight(${CURRENT_HIGHLIGHT})`);
    });
  }
});
