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
  'app frames': resolve(here, '../../../../lucidos-agents-api/src/api/sdk_iframe.css'),
};

describe('the find highlights are styled under the names the matcher paints', () => {
  for (const [who, path] of Object.entries(SHEETS)) {
    it(`for ${who}`, () => {
      const css: string = readFileSync(path, 'utf-8');
      expect(css).toContain(`::highlight(${ALL_HIGHLIGHT})`);
      expect(css).toContain(`::highlight(${CURRENT_HIGHLIGHT})`);
    });
  }

  it('announces the app-frame rules through the property the SDK reads', () => {
    const css: string = readFileSync(SHEETS['app frames'], 'utf-8');
    expect(css).toMatch(/:root\s*\{\s*--find-highlights:\s*styled;\s*\}/);
  });
});
