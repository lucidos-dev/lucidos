/**
 * A markdown link wraps at its spaces, never mid-word beside a free one.
 *
 * The regression: `.markdown-content a` set `word-break: break-all`, which
 * allows a break between any two letters. A thread link titled "Continue the
 * Loop" rendered as "Continue t" over "he Loop". The space before "the" was
 * free.
 *
 * The container's `word-break: break-word` already breaks a long URL where it
 * cannot fit, and a link inherits it. So the link sets no `word-break` of its
 * own.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const sharedCss = readFileSync(resolve(here, '../global/shared-components.css'), 'utf-8');

describe('a markdown link wraps at its spaces', () => {
  it('sets no word-break of its own', () => {
    const wordBreak = decl(block(sharedCss, '.markdown-content a {'), 'word-break');
    expect(wordBreak, `.markdown-content a sets word-break: ${wordBreak}`).toBeNull();
  });

  it('inherits a container value that still breaks an over-wide URL', () => {
    const wordBreak = decl(block(sharedCss, '.markdown-content {'), 'word-break');
    expect(wordBreak).toBe('break-word');
  });
});
