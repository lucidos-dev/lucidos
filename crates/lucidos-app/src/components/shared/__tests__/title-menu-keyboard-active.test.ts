import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const cssSource = readFileSync(resolve(here, '../../../styles/mobile.css'), 'utf-8');
const drawerCss = readFileSync(resolve(here, '../../../styles/drawer.css'), 'utf-8');

describe('Mobile title-row menu button: tappability', () => {
  it('elevates .thread-content above .edge-swipe-zone via z-index', () => {
    expect(cssSource).toMatch(
      /\.mobile-swipe-pane\s+\.thread-content\s*\{[^}]*z-index:\s*2\s*;/,
    );
  });

  it('keeps the keyboard-active block on .mobile-thread-title-row', () => {
    expect(cssSource).toMatch(
      /:root\[data-keyboard-active\]\s+\.mobile-thread-title-row\s*\{\s*pointer-events:\s*none/,
    );
  });

  it('re-enables pointer-events on the title menu button when keyboard is active', () => {
    expect(cssSource).toMatch(
      /:root\[data-keyboard-active\]\s+\.mobile-thread-title-row\s+\.thread-title-menu\s*\{\s*pointer-events:\s*auto/,
    );
  });

  it('selects no text on a hold', () => {
    const rule = drawerCss.match(/(^|\})\s*\.thread-title-menu\s*\{([^}]*)\}/m)?.[2] ?? '';
    expect(rule).toMatch(/-webkit-user-select:\s*none/);
    expect(rule).toMatch(/(^|[^-])user-select:\s*none/);
  });
});
