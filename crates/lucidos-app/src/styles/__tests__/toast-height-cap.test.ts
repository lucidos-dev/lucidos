import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');
const mobileCss = readFileSync(resolve(here, '../mobile.css'), 'utf-8');

const ABSOLUTE_CAP_CEILING_REM = 16;

function absoluteCapRem(maxHeight: string | null): number | null {
  const m = maxHeight?.match(/(\d+(?:\.\d+)?)rem\s*,/);
  return m ? Number(m[1]) : null;
}

function isCapped(maxHeight: string | null): boolean {
  const rem = absoluteCapRem(maxHeight);
  return rem !== null && rem <= ABSOLUTE_CAP_CEILING_REM;
}

describe('a long toast body cannot swallow the viewport', () => {
  it('caps .toast at an absolute height, not only a viewport-derived one', () => {
    const maxHeight = decl(block(componentsCss, '.toast {'), 'max-height');
    expect(maxHeight).not.toBeNull();

    expect(maxHeight!.startsWith('min(')).toBe(true);

    expect(absoluteCapRem(maxHeight), `no absolute rem cap in "${maxHeight}"`).not.toBeNull();
    expect(absoluteCapRem(maxHeight)!).toBeLessThanOrEqual(ABSOLUTE_CAP_CEILING_REM);

    expect(maxHeight).toContain('--app-height');
  });

  it('is capped everywhere, with no sheet raising it back', () => {
    const raisers = [...rulesTargeting(componentsCss, 'toast'), ...rulesTargeting(mobileCss, 'toast')]
      .filter((rule) => rule.props.has('max-height'))
      .filter((rule) => !isCapped(rule.props.get('max-height')!));

    expect(
      raisers.map((r) => `${r.atRules} ${r.selector} { max-height: ${r.props.get('max-height')} }`),
      'a later rule re-raises the toast cap past the absolute one',
    ).toEqual([]);
  });

  it('scrolls the overflow inside the message, leaving the actions and the X reachable', () => {
    const text = block(componentsCss, '.toast-text {');
    expect(decl(text, 'overflow-y')).toBe('auto');
    expect(decl(text, 'min-height')).toBe('0');
    const heading = block(componentsCss, '.toast-heading {');
    expect(decl(heading, 'overflow-y')).toBe('auto');
    expect(decl(block(componentsCss, '.toast-body {'), 'min-height')).toBe('0');
    const toast = block(componentsCss, '.toast {');
    expect(decl(toast, 'display')).toBe('flex');
    expect(decl(toast, 'flex-direction')).toBe('column');
  });

  it('floors the heading at the close button, and sizes the button from it', () => {
    const heading = block(componentsCss, '.toast-heading {');
    expect(decl(heading, 'min-height')).toBe('var(--toast-close-size)');
    expect(decl(heading, 'margin-right')).toBe('var(--toast-close-size)');

    const close = block(componentsCss, '.toast-close {');
    expect(decl(close, 'width')).toBe('var(--toast-close-size)');
    expect(decl(close, 'height')).toBe('var(--toast-close-size)');
  });

  it('bounds the heading in whole lines, and hands the squeeze downwards', () => {
    const heading = block(componentsCss, '.toast-heading {');

    expect(decl(heading, 'flex')).toBe('0 0 auto');

    const lineHeight = decl(heading, 'line-height');
    expect(lineHeight, 'a `normal` line height varies by font, so nothing divides')
      .toBe('var(--toast-line-height)');

    const maxHeight = decl(heading, 'max-height');
    expect(maxHeight).toMatch(/^calc\(\d+em \* var\(--toast-line-height\)\)$/);

    expect(decl(block(componentsCss, '.toast {'), '--toast-line-height')).not.toBeNull();

    const text = block(componentsCss, '.toast-text {');
    expect(decl(text, 'flex')).toBe('1 1 auto');
  });

  it('runs the scroll box to the toast content edge', () => {
    for (const sheet of [componentsCss, mobileCss]) {
      const offenders = rulesTargeting(sheet, 'toast-text')
        .filter((rule) =>
          ['padding-right', 'margin-right', 'padding', 'margin', 'border-right', 'width']
            .some((p) => rule.props.has(p)),
        );

      expect(
        offenders.map((r) => `${r.atRules} ${r.selector} { ${r.body} }`),
        'a right-hand reserve pulls the scrollbar back inside the text column',
      ).toEqual([]);
    }
  });

  it('keeps the spinner out of every scroll box', () => {
    const icon = block(componentsCss, '.toast-icon {');
    expect(decl(icon, 'position')).toBe('absolute');
    expect(decl(icon, 'pointer-events')).toBe('none');

    for (const sheet of [componentsCss, mobileCss]) {
      const offenders = [...rulesTargeting(sheet, 'toast-body'), ...rulesTargeting(sheet, 'toast')]
        .filter((rule) => ['overflow', 'overflow-y'].some((p) => rule.props.has(p)))
        .filter((rule) => rule.props.get('overflow') !== 'hidden');

      expect(
        offenders.map((r) => `${r.atRules} ${r.selector} { ${r.body} }`),
        'an ancestor of the icon owns a scroll the spinner would then have to switch off',
      ).toEqual([]);
    }
  });
});
