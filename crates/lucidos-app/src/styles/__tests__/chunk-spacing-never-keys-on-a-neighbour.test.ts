/**
 * No chunk spacing keys on the row next to it.
 *
 * Each prose chunk and each run of steps rolls in its own `<Disclosure>` row
 * (`ChatExchange.tsx`), and the toggles add and remove those rows. A rule that
 * reads a neighbour, such as `.disclosure + .disclosure .response-chunk`,
 * changes the gap only once a rolling row has left. The page then jumps as
 * the roll lands. The hidden-step boundary is a row of its own instead,
 * `.response-elision`, which rolls in while the steps roll out.
 *
 * A source scan because `tsc` does not read CSS and `vite build` only parses it.
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
const css: string = readFileSync(resolve(here, '../chat/response.css'), 'utf8');
const exchange: string = readFileSync(resolve(here, '../../components/chat/ChatExchange.tsx'), 'utf8');

describe('chunk spacing', () => {
  const chunkRules = cssRules(css).map((r) => r.selector).filter((s) => s.includes('response-chunk'));

  it('keeps no rule that reads a sibling row', () => {
    expect(chunkRules.length).toBeGreaterThan(0);
    for (const selector of chunkRules) {
      expect(selector, selector).not.toMatch(/[+~]|:has\(/);
    }
  });

  it('draws the hidden-step mark as a row of its own', () => {
    const selectors = cssRules(css).map((r) => r.selector);
    expect(selectors).toContain('.response-elision::before');
    expect(exchange).toContain('class="response-elision"');
  });
});
