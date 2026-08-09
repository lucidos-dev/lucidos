/**
 * The prompt-bar popover scrolls its BODY, never its shell.
 *
 * The shell's first child is a title bar (`.prompt-bar-popover-head`), and a
 * classic (space-taking) scrollbar is laid out inside its scroll container's
 * content box. So while the shell carried `overflow-y: auto`, the scrollbar
 * narrowed every child: the title bar's background and bottom border stopped
 * short of the panel's edge and the scrollbar column ran the full height past
 * it, straight through the bar.
 *
 * Neither `tsc` nor `vite build` can see this, and no default-setting Mac or
 * phone can either, because overlay scrollbars take no space and the panel
 * looks correct. Only a classic-scrollbar platform shows it, which is the same
 * reason `transcript-fade-scroll-gutter-guard.test.ts` is a source scan rather
 * than a browser e2e.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { block, decl } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const SHELL_CSS = readFileSync(resolve(here, '../chat/prompt-bar-popover.css'), 'utf8');

describe('prompt-bar popover scroll ownership', () => {
  it('keeps the shell unscrolled so its title bar spans the full panel width', () => {
    const shell = block(SHELL_CSS, '\n.prompt-bar-popover {');
    expect(decl(shell, 'overflow'), 'the shell must clip, not scroll').toBe('hidden');
    expect(decl(shell, 'overflow-y'), 'a scrolling shell narrows its own title bar').toBeNull();
    // The column is what lets the body take the leftover height and scroll it.
    expect(decl(shell, 'display')).toBe('flex');
    expect(decl(shell, 'flex-direction')).toBe('column');
  });

  it('puts the scrollbar in the body, and lets the body shrink enough to use it', () => {
    const body = block(SHELL_CSS, '\n.prompt-bar-popover-body {');
    expect(decl(body, 'overflow-y')).toBe('auto');
    // Without this the flex item's automatic minimum size is its content, so a
    // long list grows the shell past its max-height instead of scrolling.
    expect(decl(body, 'min-height')).toBe('0');
  });
});
