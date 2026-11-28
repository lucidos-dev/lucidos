import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const stylesRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const css = (rel: string) => readFileSync(resolve(stylesRoot, rel), 'utf-8');

function ruleBody(source: string, selector: string): string {
  const at = source.indexOf(`${selector} {`);
  expect(at, `no rule for ${selector}`).toBeGreaterThanOrEqual(0);
  return source.slice(at, source.indexOf('}', at));
}

describe('both top reserves carry the anchor rest', () => {
  it('desktop: the .thread-content padding adds it on top', () => {
    const body = ruleBody(css('chat/input-messages.css'), '.thread-content');
    expect(body).toMatch(/padding:\s*(calc\([^;]*)?var\(--anchor-subpixel, 0px\)\)?\s+var\(--thread-pane-gutter\)/);
  });

  it('mobile: the header spacer adds it to its height', () => {
    const source = css('mobile.css');
    const at = source.lastIndexOf('.mobile-swipe-pane .thread-content::before {');
    expect(at).toBeGreaterThanOrEqual(0);
    expect(source.slice(at, source.indexOf('}', at))).toMatch(/height:[^;]*var\(--anchor-subpixel, 0px\)/);
  });
});
