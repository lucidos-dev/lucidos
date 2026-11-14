/**
 * The `<Disclosure>` box (`components/shared/Disclosure.tsx`) measures its body
 * and rolls its own height to match. Two CSS properties keep that honest.
 *
 * - The body contains its children's margins. Otherwise a markdown body's
 *   margin collapses out of the measured height: the roll clips it, then jumps.
 * - The box never shrinks. The mid-roll clip drops a flex item's automatic
 *   minimum height, so a capped flex scroller squashed the box to nothing.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const RULES = cssRules(readFileSync(resolve(here, '../global/host-components.css'), 'utf8'));

function prop(selector: string, name: string): string | undefined {
  return RULES.find(r => r.selector === selector && r.props.has(name))?.props.get(name);
}

describe('the disclosure box', () => {
  it('holds its children\'s margins inside the body it measures', () => {
    expect(prop(':where(.disclosure-body)', 'display')).toBe('flow-root');
  });

  it('never shrinks inside a flex container', () => {
    expect(prop('.disclosure', 'flex-shrink')).toBe('0');
  });

  it('clips only while rolling', () => {
    expect(prop('.disclosure', 'overflow')).toBeUndefined();
    expect(prop('.disclosure.is-rolling', 'overflow')).toBe('hidden');
  });
});
