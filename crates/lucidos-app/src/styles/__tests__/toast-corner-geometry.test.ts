/**
 * The toast stack is ONE stack, in the window's bottom-right corner on desktop
 * and below the header on a phone. The corner is decision 6 of
 * docs/plans/2026-08-13-toast-banner-dialog-taxonomy.md. This pins its
 * geometry, so a stray shape rule or a per-pane column cannot come back
 * unnoticed.
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
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');
const shellCss = readFileSync(resolve(here, '../panels/shell.css'), 'utf-8');

const desktopCorner = componentsCss.indexOf('@media (--desktop-layout)', componentsCss.indexOf('Bottom-right corner'));

describe('the toast corner', () => {
  it('stacks toasts in one column, below the header by default', () => {
    const column = block(componentsCss, '.toast-column {');
    expect(decl(column, 'display')).toBe('flex');
    expect(decl(column, 'flex-direction')).toBe('column');

    const container = block(componentsCss, '.toast-container {');
    expect(decl(container, 'top')).toContain('var(--app-header-bottom)');
  });

  it('anchors the desktop stack to the window corner, newest nearest it', () => {
    expect(desktopCorner).toBeGreaterThan(0);
    const container = block(componentsCss, '.toast-container {', desktopCorner);
    expect(decl(container, 'top')).toBe('auto');
    expect(decl(container, 'left')).toBe('auto');
    expect(decl(container, 'bottom')).toBe('max(1rem, var(--safe-area-bottom))');
    expect(decl(container, 'right')).toBe('max(1rem, var(--safe-area-right))');
    expect(decl(container, 'width')).toBe('min(var(--toast-max-width), calc(100vw - 2rem))');

    expect(decl(block(componentsCss, '.toast-container .toast-column {', desktopCorner), 'flex-direction'))
      .toBe('column-reverse');
    expect(decl(block(componentsCss, '.toast-container .toast {', desktopCorner), 'width')).toBe('100%');
  });

  it('keeps no other shape and no per-pane column', () => {
    expect(componentsCss).not.toContain('data-toast-placement');
    expect(componentsCss).not.toContain('data-toast-pane');
    // A window-anchored stack never moves with the divider, so it has no place
    // on the drag kill list.
    expect(shellCss).not.toContain('.toast-column');
  });
});
