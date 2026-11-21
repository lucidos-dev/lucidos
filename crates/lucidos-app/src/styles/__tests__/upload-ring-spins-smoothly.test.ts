/**
 * The pending-upload ring turns without a hitch.
 *
 * `@keyframes spin` declares only `to { transform: rotate(360deg) }`, so its
 * start frame is the element's own transform. The ring once carried
 * `rotate(-90deg)` to start its arc at twelve o'clock. Each loop then turned
 * 450 degrees and snapped back 90, which read as a jerk on Waiting and Retrying.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const STYLES_DIR: string = resolve(dirname(fileURLToPath(import.meta.url)), '..');

describe('upload ring spin', () => {
  it('sets no transform on the spinning element, which the keyframes would start from', () => {
    for (const path of styleSheetPaths(STYLES_DIR)) {
      const css: string = readFileSync(path, 'utf-8');
      for (const className of ['upload-ring', 'upload-ring-spin']) {
        for (const rule of rulesTargeting(css, className)) {
          expect(rule.props.get('transform'), `${path}: ${rule.selector}`).toBeUndefined();
        }
      }
    }
  });
});
