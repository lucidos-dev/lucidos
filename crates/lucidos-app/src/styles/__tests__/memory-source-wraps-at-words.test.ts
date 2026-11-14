/**
 * A memory source box wraps at its spaces, never mid-word.
 *
 * The regression: `.memory-source-pre` set `word-break: break-all`. A leaf
 * seven levels deep in the tree browser gets a box about 260px wide. There
 * "the import drops rows" read "the imp" over "ort drops ro" over "ws".
 *
 * `overflow-wrap: anywhere` breaks a long token, such as a URL or a hash,
 * only where it cannot fit on a line of its own.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const stylesRoot: string = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const sheets = styleSheetPaths(stylesRoot).map((path) => ({ path, css: readFileSync(path, 'utf-8') }));

/** The class every source box carries, and the one the tree browser adds. */
const SOURCE_CLASSES = ['memory-source-pre', 'tree-line-source'];

describe('a memory source box wraps at its spaces', () => {
  it('no rule lets a source box break inside a word', () => {
    for (const { path, css } of sheets) {
      for (const className of SOURCE_CLASSES) {
        for (const rule of rulesTargeting(css, className)) {
          expect(rule.props.get('word-break'), `${path}: ${rule.selector}`).not.toBe('break-all');
        }
      }
    }
  });

  it('breaks a token only when it cannot fit', () => {
    const base = sheets
      .flatMap(({ css }) => rulesTargeting(css, 'memory-source-pre'))
      .find((rule) => rule.selector === '.memory-source-pre');
    expect(base?.props.get('overflow-wrap')).toBe('anywhere');
    expect(base?.props.get('white-space')).toBe('pre-wrap');
  });
});
