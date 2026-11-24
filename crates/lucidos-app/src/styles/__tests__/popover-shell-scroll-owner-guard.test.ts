import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { block, decl } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const SHELL_CSS = readFileSync(resolve(here, '../global/anchored-popover.css'), 'utf8');

describe('anchored popover scroll ownership', () => {
  it('keeps the shell unscrolled so its title bar spans the full panel width', () => {
    const shell = block(SHELL_CSS, '\n.anchored-popover {');
    expect(decl(shell, 'overflow'), 'the shell must clip, not scroll').toBe('hidden');
    expect(decl(shell, 'overflow-y'), 'a scrolling shell narrows its own title bar').toBeNull();
    expect(decl(shell, 'display')).toBe('flex');
    expect(decl(shell, 'flex-direction')).toBe('column');
  });

  it('puts the scrollbar in the body, and lets the body shrink enough to use it', () => {
    const body = block(SHELL_CSS, '\n.anchored-popover-body {');
    expect(decl(body, 'overflow-y')).toBe('auto');
    expect(decl(body, 'min-height')).toBe('0');
  });
});
