/**
 * The executor popover's App row reads as one ordinary line of value text: the
 * mark centered on the name, the name in the same secondary as Model and
 * Effort, and the two never split across lines.
 *
 * Three CSS facts carry that, and nothing else in the gate can see any of them.
 * `tsc` skips CSS, `vite build` fails only on syntax, and a jsdom render
 * resolves no layout at all. The first two arrived as a bug report from a
 * phone.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { block, decl, cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../panels/previews.css'), 'utf8');

describe('the executor popover App row', () => {
  it('lifts the icon off the baseline the value group aligns on', () => {
    // An svg has no baseline, so the browser synthesizes one at its bottom
    // edge. Baseline-aligned, the mark's bottom sat on the text baseline and
    // its top rode above the cap.
    expect(decl(block(css, '.message-route-panel .route-value-group {'), 'align-items'))
      .toBe('baseline');
    expect(decl(block(css, '.message-route-panel .route-app-icon {'), 'align-self'))
      .toBe('center');
  });

  it('sizes the fallback mark to fit the row text line box', () => {
    // The step below the row's `--font-size-md`. At `--icon-size-md` the mark
    // overflowed the name's line box and made the App row taller than the rest.
    const body = block(css, '.message-route-panel .route-app-icon svg {');
    expect(decl(body, 'width')).toBe('var(--icon-size-sm)');
    expect(decl(body, 'height')).toBe('var(--icon-size-sm)');
  });

  it('keeps the icon and the name on one line', () => {
    // A flex line breaks on an item's full width, never on the width it could
    // shrink to. So a long name went below the icon and stranded it.
    expect(decl(block(css, '.message-route-panel .route-value-group:has(> .route-app-icon) {'), 'flex-wrap'))
      .toBe('nowrap');
    // The API-client row keeps wrapping: its thread link is a button and wants
    // a line of its own once the user-agent fills the row.
    expect(decl(block(css, '.message-route-panel .route-value-group {'), 'flex-wrap'))
      .toBe('wrap');
  });

  it('leaves a failed value its red', () => {
    // `.error-text` scores (0,2,0). A row rule painting a value secondary
    // scores at least (0,2,1) and wins. That is how the Repository row's
    // "(load failed)" came out looking like an ordinary value.
    const colouring = cssRules(css).filter(
      rule => rule.selector.includes('.message-route-panel .route-row') && rule.props.has('color'),
    );
    expect(colouring.length).toBeGreaterThan(0);
    for (const rule of colouring) {
      expect(rule.selector, `${rule.selector} must exclude .error-text`)
        .toContain(':not(.error-text)');
    }
  });
});
