/**
 * Only a toast's title is bold. The message never is, whatever its shape.
 *
 * The title is explicit (`showToast(message, type, { title })`), and
 * `.toast-title` is the one element that carries it. A bold rule on the
 * heading box, the text box or the column would make message text bold again.
 */
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

describe('only the toast title is bold', () => {
  it('makes the title bold', () => {
    expect(decl(block(componentsCss, '.toast-title {'), 'font-weight')).toBe('600');
  });

  it('sets no weight on the boxes that hold the message', () => {
    for (const sheet of [componentsCss, mobileCss]) {
      const offenders = ['toast-body', 'toast-heading', 'toast-text']
        .flatMap((cls) => rulesTargeting(sheet, cls))
        .filter((rule) => rule.props.has('font-weight'));

      expect(
        offenders.map((r) => `${r.atRules} ${r.selector} { font-weight: ${r.props.get('font-weight')} }`),
        'a weight on a message box makes untitled text bold',
      ).toEqual([]);
    }
  });
});
