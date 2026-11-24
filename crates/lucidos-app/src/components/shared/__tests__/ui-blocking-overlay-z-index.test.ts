import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import { MAX_MODAL_STACK_DEPTH } from '../../../store/overlayStack';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles = (rel: string): string =>
  readFileSync(resolve(here, '../../../styles', rel), 'utf-8');

const baseCss = styles('global/base.css');
const modalCss = styles('global/modal-overlay.css');

const TOKENS: Record<string, number> = {};
for (const m of baseCss.matchAll(/--(z-[\w-]+):\s*(\d+)\s*;/g)) {
  TOKENS[m[1]] = parseInt(m[2], 10);
}

function resolveZ(expr: string): number {
  const trimmed = expr.trim().replace(/\s*!important$/, '');
  if (/^\d+$/.test(trimmed)) return parseInt(trimmed, 10);
  const varOnly = trimmed.match(/^var\(--(z-[\w-]+)\)$/);
  if (varOnly) return TOKENS[varOnly[1]];
  const calc = trimmed.match(/^calc\(\s*var\(--(z-[\w-]+)\)\s*([+-])\s*(\d+)\s*\)$/);
  if (calc) {
    const base = TOKENS[calc[1]];
    const delta = parseInt(calc[3], 10);
    return calc[2] === '+' ? base + delta : base - delta;
  }
  throw new Error(`Unrecognized z-index expression: "${expr}"`);
}

function blockZ(css: string, selector: string): number {
  const re = new RegExp(`${selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*\\{([^}]*)\\}`);
  const block = css.match(re);
  expect(block, `selector ${selector} not found`).not.toBeNull();
  const z = block![1].match(/z-index:\s*([^;]+);/);
  expect(z, `z-index not found in ${selector}`).not.toBeNull();
  return resolveZ(z![1]);
}

describe('ui-blocking overlay z-index (only toasts above the blocker)', () => {
  const overlayZ = blockZ(modalCss, '.ui-blocking-overlay');

  it('only the toast layer sits above the blocking overlay', () => {
    expect(TOKENS['z-toast']).toBeGreaterThan(overlayZ);
  });

  it('the whole modal band stays below the blocking overlay', () => {
    expect(TOKENS['z-modal'] + MAX_MODAL_STACK_DEPTH).toBeLessThan(overlayZ);
  });

  it('the tooltip layer is pulled below the overlay while blocked', () => {
    expect(modalCss).toMatch(/:root\[data-ui-blocked\]\s+#tooltip\s*\{[^}]*display:\s*none/);
  });

  it('a pseudo-fullscreen app panel needs no override, being below the blocker', () => {
    expect(TOKENS['z-app-fullscreen']).toBeLessThan(overlayZ);
    expect(modalCss).not.toMatch(/:root\[data-ui-blocked\][^{]*\.app-ui-fullscreen/);
  });
});
