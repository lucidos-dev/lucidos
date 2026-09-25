/**
 * A disabled back / forward chevron shows its tooltip and no hover wash, in
 * every pane. Reported: the thread pane's disabled chevron showed both, while
 * the content pane's showed neither.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

const hostCss = read('../global/host-components.css');
const sharedCss = read('../global/shared-components.css');
const shellCss = read('../panels/shell.css');
const navChevron = read('../../components/shared/NavChevron.tsx');

describe('a disabled nav chevron keeps its tooltip', () => {
  it('every chevron carries the class the rule keys on', () => {
    expect(navChevron).toMatch(/const cls = `icon-btn header-icon nav-chevron/);
  });

  it('takes pointer events back from .icon-btn:disabled, outside both inert regimes', () => {
    const rules = cssRules(hostCss).filter(r => /\.nav-chevron:disabled$/.test(r.selector));
    expect(rules.length).toBe(1);
    expect(rules[0].props.get('pointer-events')).toBe('auto');
    // An inert regime sets `none` on an ancestor. A value on the element wins
    // over it, so an ungated rule keeps the chevron live behind a menu.
    for (const state of ['data-keyboard-active', 'data-overlay-open']) {
      expect(rules[0].selector).toContain(`:not([${state}])`);
    }
  });
});

describe('a disabled icon button gets no hover wash', () => {
  it.each([
    ['shared-components.css', sharedCss, '.icon-btn:hover:where(:not(:disabled))'],
    ['panels/shell.css', shellCss, '.app-header .icon-btn:hover:where(:not(:disabled))'],
  ])('%s skips disabled buttons, at unchanged specificity', (_file, css, selector) => {
    const wash = cssRules(css).find(r => r.selector === selector);
    expect(wash?.props.has('background'), `${selector} is gone or paints nothing`).toBe(true);
  });
});
