import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { block, decl } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');
const drawerCss = readFileSync(resolve(here, '../drawer.css'), 'utf8');

const rem = (value: string | null) => parseFloat(value ?? '');

describe('status mark sizes', () => {
  const dot = decl(block(css, '.progress-dot {'), 'height');
  const badge = decl(block(css, '.thread-status-question-badge {'), '--badge-height');

  it('draws the spinner at the question badge size', () => {
    const spinner = block(css, '.thread-status .mini-spinner {');
    expect(decl(spinner, 'width')).toBe(badge);
    expect(decl(spinner, 'height')).toBe(badge);
  });

  it('draws the title spinner smaller than the badge, larger than the dot', () => {
    const spinner = block(drawerCss, '.thread-title-text > .thread-status .mini-spinner {');
    const width = decl(spinner, 'width');
    expect(decl(spinner, 'height')).toBe(width);
    expect(rem(width)).toBeLessThan(rem(badge));
    expect(rem(width)).toBeGreaterThan(rem(dot));
  });

  it('draws the drawer row and the pane title with one spinner rule', () => {
    expect(drawerCss).toMatch(
      /\.thread-row-title-text > \.thread-status \.mini-spinner,\s*\.thread-title-text > \.thread-status \.mini-spinner \{/,
    );
  });

  it('draws the change glyph at the question badge size', () => {
    const glyph = block(css, '.thread-status-changes-icon svg {');
    expect(decl(glyph, 'width')).toBe(badge);
    expect(decl(glyph, 'height')).toBe(badge);
  });

  it('draws the pause glyph as tall as the dot', () => {
    expect(decl(block(css, '.thread-status-paused-icon svg {'), 'height')).toBe(dot);
  });
});
