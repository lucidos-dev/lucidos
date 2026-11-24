import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, cssRules, decl } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');
const mobileCss = readFileSync(resolve(here, '../mobile.css'), 'utf-8');

const BREAKING = ['anywhere', 'break-word'];

const MESSAGE_SUBTREE_RE =
  /\.toast-(body|heading|title|text)\b/;

describe('a toast message wraps a long path', () => {
  it('breaks an over-wide token in the message column', () => {
    const wrap = decl(block(componentsCss, '.toast-body {'), 'overflow-wrap');
    expect(BREAKING, `.toast-body sets overflow-wrap: ${wrap}`).toContain(wrap);
  });

  it('lets nothing in the message subtree opt back out of wrapping', () => {
    const offenders = [componentsCss, mobileCss].flatMap((sheet) =>
      cssRules(sheet)
        .filter((rule) => MESSAGE_SUBTREE_RE.test(rule.selector))
        .filter((rule) => rule.props.get('overflow-wrap') === 'normal'),
    );

    expect(
      offenders.map((r) => `${r.atRules} ${r.selector} { ${r.body} }`),
      'a rule stops the message wrapping, so a long path overflows again',
    ).toEqual([]);
  });
});
