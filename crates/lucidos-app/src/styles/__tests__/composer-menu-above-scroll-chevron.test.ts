import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles = (rel: string): string => readFileSync(resolve(here, '..', rel), 'utf-8');

const LIFT = ':has([data-overlay-panel]):not(:has(.modal-overlay))';
const sheets = [styles('panels/content.css'), styles('mobile.css')];
const rules = sheets.flatMap(cssRules);

function floatOffset(value: string | undefined): number {
  const m = value?.match(/^calc\(var\(--z-float\) \+ (\d+)\)$/);
  expect(m, `expected calc(var(--z-float) + n), got ${value}`).not.toBeNull();
  return Number(m![1]);
}

describe('composer menu above the scroll chevron', () => {
  const chevron = cssRules(styles('chat/input-messages.css'))
    .find(r => r.selector === '.scroll-to-top, .scroll-to-bottom, .follow-toggle');
  const chevronOffset = floatOffset(chevron?.props.get('z-index'));

  it('lifts every z-indexed composer above the chevron while a menu is open', () => {
    const zIndexed = rules.filter(r =>
      /\.prompt-area$/.test(r.selector) && r.props.has('z-index') && !r.selector.includes(','));
    expect(zIndexed.length).toBeGreaterThan(0);
    for (const base of zIndexed) {
      const lift = rules.find(r => r.selector === `${base.selector}${LIFT}`);
      expect(lift, `no lift for \`${base.selector}\``).toBeDefined();
      expect(floatOffset(lift!.props.get('z-index'))).toBeGreaterThan(chevronOffset);
    }
  });

  it('drops the follow toggle beneath an open menu', () => {
    const drop = cssRules(styles('chat/input-messages.css'))
      .find(r => r.selector === `.prompt-area${LIFT} .follow-toggle`);
    expect(drop?.props.get('z-index')).toBe('auto');
  });
});
