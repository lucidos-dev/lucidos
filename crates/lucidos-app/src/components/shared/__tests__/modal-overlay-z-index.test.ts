import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../../../styles/global.css'), 'utf-8');

/**
 * Regression test: a modal overlay (StepDetailModal, NotificationsModal,
 * ScaleModal, …) MUST sit above the floating header chrome so it actually
 * blocks the rest of the UI. Previous bug: --z-modal was 2000 while
 * --z-control-panel was 2200, so header buttons (compose, search, menu,
 * thread nav, title) punched through the dim backdrop. Hovering them
 * showed tooltips, clicking them ran their action instead of closing the
 * modal, and the modal failed its core promise of "next click outside
 * closes it".
 */
describe('modal overlay z-index (regression: header punch-through)', () => {
  function tokenValue(name: string): number {
    const m = css.match(new RegExp(`--${name}:\\s*(\\d+)\\s*;`));
    expect(m, `token --${name} not found in global.css :root`).not.toBeNull();
    return parseInt(m![1], 10);
  }

  it('--z-modal must sit strictly above --z-control-panel so the dim backdrop covers the header', () => {
    expect(tokenValue('z-modal')).toBeGreaterThan(tokenValue('z-control-panel'));
  });

  it('--z-toast must stay strictly above --z-modal so toasts remain visible over open modals', () => {
    expect(tokenValue('z-toast')).toBeGreaterThan(tokenValue('z-modal'));
  });
});
