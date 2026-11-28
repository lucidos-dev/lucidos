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
