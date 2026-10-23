import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

// The rung control sheds on a narrow bar before anything folds into the ⋯
// menu: the effort first, which brings in the intelligence badge, then the
// model name, leaving the badge. The row's fold measures and stamps the level
// as `data-shed`, so no width breakpoint lives here.
const here: string = dirname(fileURLToPath(import.meta.url));
const css: string = readFileSync(resolve(here, '../chat/rung-control.css'), 'utf-8').replace(/\/\*[\s\S]*?\*\//g, '');

/** The selectors of the rule whose body is exactly `body`. */
function selectorsOf(body: string): string[] {
  const hit = [...css.matchAll(/(^|\n)([^{}\n][^{}]*?) \{\s*([^}]*?)\s*\}/g)].find((m) => m[3] === body);
  if (!hit) throw new Error(`no rule is exactly ${body}`);
  return hit[2].split(',').map((s) => s.trim());
}

describe('the rung control sheds on a narrow bar', () => {
  it('keeps no width breakpoint: the fold decides', () => {
    expect(css).not.toContain('@container');
  });

  it('sheds the effort at level 1 and the name at level 2', () => {
    expect(selectorsOf('display: none;')).toEqual([
      '[data-shed="1"] .rung-effort',
      '[data-shed="2"] .rung-effort',
      '[data-shed="2"] .rung-face:has(> .rung-intelligence) .rung-name',
    ]);
  });

  it('shows the intelligence badge only once the pill sheds', () => {
    expect(css).toMatch(/\n\.rung-intelligence \{[^}]*display: none;/);
    expect(selectorsOf('display: inline-grid;')).toEqual([
      '[data-shed="1"] .rung-intelligence',
      '[data-shed="2"] .rung-intelligence',
    ]);
  });

  it('stacks every band in the badge, so each badge is as wide as the widest', () => {
    expect(css).toMatch(/\.rung-intelligence > span \{\s*grid-area: 1 \/ 1;/);
    expect(css).toMatch(/\.rung-intelligence-reserve \{\s*visibility: hidden;/);
    expect(css).toMatch(/\n\.rung-intelligence \{[^}]*justify-items: center;/);
  });

  it('never hides the caret', () => {
    expect(css).not.toMatch(/\.rung-trigger[^{]*\{[^}]*display: none/);
  });

  it('stacks every face in one cell, so a step never resizes the pill', () => {
    expect(css).toMatch(/\.rung-label \{\s*display: grid;/);
    expect(css).toMatch(/\.rung-face \{[^}]*grid-area: 1 \/ 1;/);
    expect(css).toMatch(/\.rung-face-reserve \{\s*visibility: hidden;/);
  });

  it('overhangs the send button equally above and below, so the row stays one height', () => {
    expect(css).toContain('margin-block: calc((var(--send-round-size) - var(--rung-height)) / 2);');
  });

  it('is two rem on a phone', () => {
    expect(css).toMatch(/@media \(--phone-layout\) \{\s*\.rung-control \{\s*--rung-height: 2rem;/);
  });

  it('keeps both steps on a phone, only smaller', () => {
    const phone = [...css.matchAll(/@media \(--phone-layout\) \{([\s\S]*?)\n\}/g)].map((m) => m[1]).join('\n');
    expect(css).not.toMatch(/\.rung-step[^{]*\{[^}]*display: none/);
    expect(css).toMatch(/\.rung-pill \.rung-step \{[^}]*width: 1\.5rem;/);
    expect(phone).toMatch(/\.rung-pill \.rung-step \{\s*width: 1\.25rem;/);
  });

  it('draws one capsule, with a divider each side of the trigger', () => {
    const rule = (selector: string) =>
      css.match(new RegExp(`${selector.replace(/[.[\]"]/g, '\\$&')} \\{([^}]*)\\}`))?.[1] ?? '';
    expect(rule('.rung-pill')).toContain('border: 1px solid var(--border-color);');
    expect(rule('.rung-pill button')).toContain('border: 0;');
    expect(rule('.rung-pill .rung-trigger')).toContain('border-inline: 1px solid var(--border-color);');
  });
});
