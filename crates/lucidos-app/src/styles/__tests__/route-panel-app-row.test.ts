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
    expect(decl(block(css, '.message-route-panel .route-value-group {'), 'align-items'))
      .toBe('baseline');
    expect(decl(block(css, '.message-route-panel .route-app-icon {'), 'align-self'))
      .toBe('center');
  });

  it('sizes the app icon to fit the row text line box', () => {
    const body = block(css, '.message-route-panel .route-app-icon {');
    expect(decl(body, '--app-icon-size')).toBe('var(--icon-size-sm)');
  });

  it('keeps the icon and the name on one line', () => {
    expect(decl(block(css, '.message-route-panel .route-value-group:has(> .route-app-icon) {'), 'flex-wrap'))
      .toBe('nowrap');
    expect(decl(block(css, '.message-route-panel .route-value-group {'), 'flex-wrap'))
      .toBe('wrap');
  });

  it('leaves a failed value its red', () => {
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
