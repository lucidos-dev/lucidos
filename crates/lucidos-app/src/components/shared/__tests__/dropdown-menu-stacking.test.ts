import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

const baseCss: string = read('../../../styles/global/base.css');
const menuCss: string = read('../../../styles/global/shared-components.css');
const dropdownSource: string = read('../Dropdown.tsx');

const TOKENS: Record<string, number> = {};
for (const m of baseCss.matchAll(/--(z-[\w-]+):\s*(\d+)\s*;/g)) {
  TOKENS[m[1]] = parseInt(m[2], 10);
}

function resolveZ(expr: string): number {
  const trimmed = expr.trim();
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

describe('dropdown menu stacking (regression: header icons painted over the menu)', () => {
  const menuZ = blockZ(menuCss, '.dropdown-menu');

  it('out-ranks the floating header chrome', () => {
    expect(menuZ).toBeGreaterThan(TOKENS['z-control-panel']);
  });

  it('stays below the modal layer so a modal still covers an open menu', () => {
    expect(menuZ).toBeLessThan(TOKENS['z-modal']);
  });

  it('is portaled to <body>, without which the z-index is capped by the pane', () => {
    expect(dropdownSource).toMatch(/\n\s*portal\n/);
  });

  it('leaves the header popout token restore unnecessary by construction', () => {
    const shellCss: string = read('../../../styles/panels/shell.css');
    expect(shellCss).not.toMatch(/\.app-header\s+\.dropdown-menu\s*[,{]/);
  });
});
