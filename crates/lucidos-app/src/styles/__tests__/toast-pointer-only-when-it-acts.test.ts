/**
 * A toast card shows the pointer only when a click on it acts.
 *
 * A passive info or success toast still closes on a click (`toastTap` answers
 * `'dismiss'`), but the pointer promised a destination that did not exist. So
 * the pointer belongs to `click` and `action` cards, never to a dismiss.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, selectorList } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');

const pointerSelectors = cssRules(componentsCss)
  .filter((rule) => rule.props.get('cursor') === 'pointer')
  .flatMap((rule) => selectorList(rule.selector))
  .filter((s) => s.includes('.toast[data-toast-tap'));

describe('toast card cursor', () => {
  it('points on a card that acts', () => {
    expect(pointerSelectors).toContain('.toast[data-toast-tap="click"]');
    expect(pointerSelectors).toContain('.toast[data-toast-tap="action"]');
  });

  it('does not point on a card that only dismisses', () => {
    expect(pointerSelectors).not.toContain('.toast[data-toast-tap]');
    expect(pointerSelectors).not.toContain('.toast[data-toast-tap="dismiss"]');
  });
});
