import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl, rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const css: string = readFileSync(resolve(here, '../settings/response-style-editor.css'), 'utf-8');
const everySheet: string[] = styleSheetPaths(resolve(here, '..'))
  .map((p: string) => readFileSync(p, 'utf-8'));

describe('the response style modal shows as much instruction as it can', () => {
  it('takes all the width bar a single gutter', () => {
    const width = decl(block(css, '.style-editor-modal {'), 'width');
    expect(width).not.toBeNull();
    expect(width!.startsWith('min(')).toBe(true);
    expect(width).toContain('100vw');
  });

  it('is imported, so every rule here actually ships', () => {
    const entry: string = readFileSync(resolve(here, '../settings.css'), 'utf-8');
    expect(entry).toContain("@import './settings/response-style-editor.css';");
  });

  it('gives the panel a definite height, bounded by the VISIBLE viewport', () => {
    const panel = block(css, '.style-editor-modal {');
    expect(decl(panel, '--style-editor-room')).toContain('--app-height');
    expect(decl(panel, 'height')).toContain('var(--style-editor-room)');
    expect(decl(panel, 'max-height'), 'a max-height sizes to content instead')
      .toBeNull();
  });

  it('drops the desktop ceiling on a phone, leaving the screen as the only bound', () => {
    const mobile = rulesTargeting(css, 'style-editor-modal')
      .filter((rule) => rule.atRules.startsWith('@media'));

    expect(mobile, 'no mobile rule releases the ceiling').toHaveLength(1);
    expect(mobile[0].props.get('height')).toBe('var(--style-editor-room)');
  });

  it('hands that leftover height to the instruction, which scrolls inside it', () => {
    const field = block(css, '.style-editor-instruction {');
    expect(decl(field, 'overflow-y')).toBe('auto');
    expect(decl(field, 'resize')).toBe('none');

    const grow = block(css, '.style-editor-field-grow {');
    expect(decl(grow, 'min-height')).toBe('0');
    expect(decl(grow, 'flex')).toBe('1 1 auto');

    const panel = block(css, '.style-editor-modal {');
    expect(decl(panel, 'display')).toBe('flex');
    expect(decl(panel, 'flex-direction')).toBe('column');
  });

  it('keeps Save reachable on a screen too short for the layout', () => {
    const panel = block(css, '.style-editor-modal {');
    expect(decl(panel, 'overflow-y'), 'a clipped panel loses its buttons')
      .toBe('auto');
    expect(decl(panel, 'overflow'), 'a shorthand would clip the other axis')
      .toBeNull();

    expect(decl(block(css, '.style-editor-modal > * {'), 'flex-shrink')).toBe('0');
    expect(css.indexOf('.style-editor-modal > *'))
      .toBeLessThan(css.indexOf('.style-editor-field-grow {'));
  });

  it('is not un-scrolled by any later sheet', () => {
    const raisers = everySheet
      .flatMap((sheet: string) => rulesTargeting(sheet, 'style-editor-instruction'))
      .filter((rule) => ['overflow', 'overflow-y'].some((p) => rule.props.has(p)))
      .filter((rule) => rule.props.get('overflow-y') !== 'auto');

    expect(
      raisers.map((r) => `${r.atRules} ${r.selector} { ${r.body} }`),
      'a later rule takes the scroll off the instruction field',
    ).toEqual([]);
  });
});
