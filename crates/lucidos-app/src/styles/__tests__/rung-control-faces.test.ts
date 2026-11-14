import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

// The rung control never resizes: its label has one width per face set. The
// desktop face shows the full pill, the compact face the intelligence badge
// alone. The compact face is the phone layout or a narrow composer row, both
// marked `data-composer-compact` on the row. Lack of room folds the pill, so no
// width breakpoint lives here.
const here: string = dirname(fileURLToPath(import.meta.url));
const css: string = readFileSync(resolve(here, '../chat/rung-control.css'), 'utf-8').replace(/\/\*[\s\S]*?\*\//g, '');
const PHONE_BLOCK = /@media \(--phone-layout\) \{[\s\S]*?\n\}/g;
const COMPACT_RULE = /(?:^|\n)((?:\[data-composer-compact\][^{}]*?)) \{([^}]*)\}/g;
const phone = [...css.matchAll(PHONE_BLOCK)].map((m) => m[0]).join('\n');
/** The compact rules, with the attribute prefix dropped so `rule` finds them. */
const compact = [...css.matchAll(COMPACT_RULE)]
  .map((m) => `${m[1].split('[data-composer-compact] ').join('')} {${m[2]}}`).join('\n');
const desktop = css.replace(PHONE_BLOCK, '').replace(COMPACT_RULE, '');

/** The body of the first rule for exactly `selector` in `source`. */
function rule(source: string, selector: string): string {
  return source.match(new RegExp(`(?:^|\\n)\\s*${selector.replace(/[.[\]"()>:]/g, '\\$&')} \\{([^}]*)\\}`))?.[1] ?? '';
}

describe('the rung control keeps one face per face set', () => {
  it('keeps no width breakpoint and no shedding level', () => {
    expect(css).not.toContain('@container');
    expect(css).not.toContain('data-shed');
  });

  it('gives the label one width per face set', () => {
    expect(rule(css, '.rung-label')).toContain('width: var(--rung-label-width);');
    expect(rule(desktop, '.rung-control')).toMatch(/--rung-label-width: [\d.]+rem;/);
    expect(rule(compact, '.rung-control')).toMatch(/--rung-label-width: [\d.]+rem;/);
  });

  it('ends a long name in an ellipsis and keeps the effort whole', () => {
    expect(rule(css, '.rung-label')).toContain('overflow: hidden;');
    expect(rule(css, '.rung-name')).toContain('text-overflow: ellipsis;');
    expect(rule(css, '.rung-effort')).toContain('flex-shrink: 0;');
  });

  it('hides the badge on the desktop face', () => {
    expect(rule(desktop, '.rung-intelligence')).toContain('display: none;');
  });

  it('shows the badge alone on the compact face, as plain text in a bare frame', () => {
    expect(rule(compact, '.rung-intelligence')).toContain('display: inline;');
    const badge = rule(css, '.rung-intelligence');
    expect(badge).not.toContain('background');
    expect(badge).not.toContain('padding');
    const hidden = compact.match(/([^{}]+) \{\s*display: none;\s*\}/)![1].split(',').map((s) => s.trim());
    expect(hidden).toEqual([
      '.rung-effort',
      '.rung-label:has(> .rung-intelligence) .rung-name',
      '.rung-pill .rung-step',
      '.rung-trigger > svg',
    ]);
    expect(rule(compact, '.rung-pill .rung-trigger')).toContain('border-inline: 0;');
  });

  it('overhangs the send button equally above and below, so the row stays one height', () => {
    expect(css).toContain('margin-block: calc((var(--send-round-size) - var(--rung-height)) / 2);');
  });

  it('is two rem on a phone, and only there', () => {
    expect(rule(phone, '.rung-control')).toContain('--rung-height: 2rem;');
    expect(compact).not.toContain('--rung-height');
  });

  it('reads the face set from the row, in no media query', () => {
    expect(phone).not.toContain('.rung-label');
    expect(phone).not.toContain('.rung-intelligence');
  });

  it('draws one capsule, with a divider each side of the trigger', () => {
    expect(rule(css, '.rung-pill')).toContain('border: 1px solid var(--border-color);');
    expect(rule(css, '.rung-pill button')).toContain('border: 0;');
    expect(rule(desktop, '.rung-pill .rung-trigger')).toContain('border-inline: 1px solid var(--border-color);');
  });
});
