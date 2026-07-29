import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const settingsCss = readFileSync(resolve(here, '../../styles/settings/base.css'), 'utf-8');
const pagesCss = readFileSync(resolve(here, '../../styles/pages.css'), 'utf-8');

function getBlock(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const re = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'g');
  return [...css.matchAll(re)].map(m => m[1]).join('\n');
}

function declarationValue(block: string, property: string): string | undefined {
  const escaped = property.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return block.match(new RegExp(`${escaped}\\s*:\\s*([^;]+)`))?.[1].trim();
}

// The navigation focus marker (.nav-focus-stuck, styles/global/host-components.css)
// is an outline offset just beyond the marked element's padding box, so the gap it
// shows on each side equals that side's padding. A settings row and a plugin row
// are padded asymmetrically (settings: 0.5rem top/bottom, 0 sides; plugin/list-row:
// 0.5rem top/bottom, 1rem sides), so each gets a scoped rule that normalizes all
// four sides to one gap with cancelling margins (no reflow): settings shrinks to
// 0.375rem (the largest gap whose outline still fits the clip container), plugins
// to their 0.5rem vertical. These rules MUST stay attached to the current marker
// class; a rename that leaves them on the old class silently drops the rule and the
// gap regresses (the same failure mode the chat rule's guard protects against).
describe('nav focus marker — uniform gap outside chat', () => {
  it('gives a focus-marked settings row a uniform gap that fits its clip container', () => {
    const block = getBlock(settingsCss, '.settings-row.nav-focus-stuck');
    expect(block).not.toBe('');
    // Uniform 0.375rem gap on all four sides: sides grown from 0 (margin cancels),
    // the 0.5rem feed padding shrunk to 0.375rem with the 0.125rem handed back as
    // margin. 0.375 (not 0.5) because the gap + the marker's 0.375rem outline reach
    // must fit within .settings-panel's --space-lg (1rem) side room — a pinned
    // marker-geometry value; historically a 0.5rem grow overran the then-0.75rem
    // gutter by 0.125rem and clipped the outline against .content-pane-body's overflow-x.
    expect(declarationValue(block, 'padding')).toBe('0.375rem');
    expect(declarationValue(block, 'margin')).toBe('0.125rem -0.375rem');
    // The vertical margin is only safe (no content shift) if the section contains
    // it — guard the BFC that makes that true.
    const section = getBlock(settingsCss, '.settings-section');
    expect(declarationValue(section, 'display')).toBe('flow-root');
  });

  it('preserves a nested settings row indent under the marker', () => {
    // A .settings-row-child is indented via padding-left: 1rem. The uniform-gap
    // rule (two classes) outranks that single-class indent, so the child needs a
    // margin-left override to shift its box by (indent - gap) = 0.625rem and keep
    // the content at its indent. Dropping this rule regresses the indent on focus.
    const block = getBlock(settingsCss, '.settings-row-child.nav-focus-stuck');
    expect(block).not.toBe('');
    expect(declarationValue(block, 'margin-left')).toBe('0.625rem');
  });

  it('shrinks a focus-marked plugin row sideways to match its vertical gap', () => {
    const block = getBlock(pagesCss, '.app-store-plugin-row.nav-focus-stuck');
    expect(block).not.toBe('');
    // Sides shrink from 1rem to the row's 0.5rem top/bottom padding, cancelled by a
    // positive margin so the stretched row keeps its content box / position.
    expect(declarationValue(block, 'padding-left')).toBe('0.5rem');
    expect(declarationValue(block, 'padding-right')).toBe('0.5rem');
    expect(declarationValue(block, 'margin-left')).toBe('0.5rem');
    expect(declarationValue(block, 'margin-right')).toBe('0.5rem');
  });

  it('has no pre-rename marker class names left behind', () => {
    expect(settingsCss).not.toMatch(/\.event-pulse|\.event-focus-stuck/);
    expect(pagesCss).not.toMatch(/\.event-pulse|\.event-focus-stuck/);
  });
});
