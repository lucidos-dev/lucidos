import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

// The rung control sheds on a narrow bar before it folds into the ⋯ menu:
// the effort first, which brings in the position badge, then the model name,
// leaving the badge. The row is the container, so the bar's own width decides,
// in rem.
const here: string = dirname(fileURLToPath(import.meta.url));
const css: string = readFileSync(resolve(here, '../chat/rung-control.css'), 'utf-8');

/** The max-width, in rem, of the `@container prompt-row` block that hides `selector`. */
function shedsBelow(selector: string): number {
  const blocks = [...css.matchAll(/@container prompt-row \(max-width: ([\d.]+)rem\) \{([\s\S]*?)\n\}/g)];
  const hit = blocks.find((b) => b[2].includes(selector) && b[2].includes('display: none'));
  if (!hit) throw new Error(`nothing sheds ${selector}`);
  return Number(hit[1]);
}

describe('the rung control sheds on a narrow bar', () => {
  it('makes the prompt row the container it measures', () => {
    expect(css).toMatch(/\.prompt-actions-row \{\s*container: prompt-row \/ inline-size;/);
  });

  it('sheds the effort before the model name', () => {
    expect(shedsBelow('.rung-effort')).toBeGreaterThan(shedsBelow('.rung-name'));
  });

  it('keeps the model name on a face with no position badge', () => {
    expect(css).toMatch(/\.rung-face:has\(> \.rung-position\) \.rung-name \{\s*display: none;/);
  });

  it('shows the position badge only once the effort sheds', () => {
    expect(css).toMatch(/\n\.rung-position \{[^}]*display: none;/);
    const effortBlock = [...css.matchAll(/@container prompt-row \(max-width: ([\d.]+)rem\) \{([\s\S]*?)\n\}/g)]
      .find((b) => Number(b[1]) === shedsBelow('.rung-effort'))!;
    expect(effortBlock[2]).toMatch(/\.rung-position \{\s*display: inline-flex;/);
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
