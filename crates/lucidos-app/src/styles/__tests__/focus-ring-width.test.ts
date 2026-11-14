/**
 * `--focus-ring-width` names the focus ring's band, so a box that clips can
 * reserve that much ring room and keep the ring whole. The ring itself stays a
 * literal, because a theme replaces `--focus-ring` whole. So the width copies
 * every definition's band. This scan holds the copies together, in the host
 * and in the stylesheet served to app frames.
 *
 * The ring room itself is checked in a browser:
 * `e2e/focus-ring-not-clipped.spec.ts`.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { focusRingWidthRem, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const iframeCssPath: string = resolve(stylesDir, '../../../lucidos-engine/src/api/sdk_iframe.css');
const read = (path: string): string => readFileSync(path, 'utf-8');

const width = focusRingWidthRem(read(resolve(stylesDir, 'global/base.css')));

describe('the focus ring width', () => {
  it('is the band of every --focus-ring definition', () => {
    const sheets = [...styleSheetPaths(stylesDir), iframeCssPath];
    let found = 0;
    for (const path of sheets) {
      for (const m of read(path).matchAll(/^\s*--focus-ring:\s*([^;]+);/gm)) {
        found++;
        const where = `${relative(stylesDir, path)}: --focus-ring: ${m[1]}`;
        const band = /^0 0 0 ([\d.]+)rem\b/.exec(m[1].trim());
        expect(band, `${where} is not a 0 0 0 <rem> band`).not.toBeNull();
        expect(parseFloat(band![1]), where).toBe(width);
      }
    }
    expect(found, 'no --focus-ring definitions found').toBeGreaterThan(1);
  });

  it('reaches app frames with the same value', () => {
    expect(focusRingWidthRem(read(iframeCssPath))).toBe(width);
  });
});
