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
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');
const surfaceCss = readFileSync(resolve(here, '../global/surface.css'), 'utf-8');
const toastTsx = readFileSync(resolve(here, '../../components/shared/Toast.tsx'), 'utf-8');
const confirmTsx = readFileSync(resolve(here, '../../components/shared/ConfirmDialog.tsx'), 'utf-8');

describe('.button-group keeps a row of buttons inside its container', () => {
  it('wraps, so buttons stack instead of overflowing', () => {
    const rule = block(sharedCss, '.button-group {');
    expect(decl(rule, 'display')).toBe('flex');
    expect(
      decl(rule, 'flex-wrap'),
      'without flex-wrap the row overflows its container instead of stacking',
    ).toBe('wrap');
  });

  it('leaves alignment to the consumer', () => {
    expect(decl(block(sharedCss, '.button-group {'), 'justify-content')).toBeNull();
  });

  it('bounds each button so an over-long label ellipsizes', () => {
    const rule = block(sharedCss, '.button-group > .action-btn {');
    expect(decl(rule, 'max-width')).toBe('100%');
    expect(decl(rule, 'overflow')).toBe('hidden');
    expect(decl(rule, 'text-overflow')).toBe('ellipsis');
    expect(
      decl(rule, 'min-width'),
      'a min-width override here would drop .action-btn\'s 3.5rem floor',
    ).toBeNull();
  });

  it('is the primitive the toast uses, not a copy the toast keeps', () => {
    expect(
      toastTsx,
      'the toast action row must opt into the shared primitive',
    ).toContain('class="toast-actions button-group"');
    const rule = block(componentsCss, '.toast-actions {');
    expect(decl(rule, 'justify-content'), 'the toast owns only its alignment').toBe('flex-end');
    for (const prop of ['display', 'flex-wrap', 'gap']) {
      expect(
        decl(rule, prop),
        `.toast-actions re-declares ${prop}; that belongs to .button-group`,
      ).toBeNull();
    }
  });

  it('bounds the surface foot, where every dialog lays out its buttons', () => {
    expect(
      confirmTsx,
      'ConfirmDialog is the surface that grew the long label',
    ).toContain('class="surface-foot"');
    const rule = block(surfaceCss, '.surface-foot {');
    expect(decl(rule, 'display')).toBe('flex');
    expect(
      decl(rule, 'flex-wrap'),
      'without flex-wrap .surface-foot overflows the dialog instead of stacking',
    ).toBe('wrap');
    expect(decl(rule, 'justify-content')).toBe('flex-end');
  });
});
