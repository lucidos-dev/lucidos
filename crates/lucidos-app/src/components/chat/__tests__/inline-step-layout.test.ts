import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import { cssRules, rulesTargeting } from '../../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));

describe('inline step layout (CSS regression)', () => {
  const css = readFileSync(resolve(here, '../../../styles/steps.css'), 'utf-8');

  function getBlock(selector: string): string {
    const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const re = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'g');
    return [...css.matchAll(re)].map(m => m[1]).join('\n');
  }

  it('inline-step must not flex-wrap (checkmark and text stay on same line)', () => {
    const block = getBlock('.inline-step');
    expect(block).not.toContain('flex-wrap: wrap');
  });

  it('step-description must truncate with ellipsis', () => {
    const block = getBlock('.inline-step .step-description');
    expect(block).toContain('overflow: hidden');
    expect(block).toContain('text-overflow: ellipsis');
  });

  it('step-detail must use flex 1 1 0 to absorb shrinkage', () => {
    const block = getBlock('.inline-step .step-detail');
    expect(block).toContain('flex: 1 1 0');
  });

  it('step-detail must be single-line, and must never be crushed to nothing', () => {
    const block = getBlock('.inline-step .step-detail');
    expect(block).toContain('white-space: nowrap');
    expect(block).toContain('text-overflow: ellipsis');
    expect(block).toContain('overflow: hidden');
    expect(block).not.toMatch(/^\s*word-break:/m);
    expect(block).not.toMatch(/^\s*min-width:\s*0/m);
    expect(block).toMatch(/^\s*min-width:\s*min\(/m);
  });

  it('step-context must not wrap or shrink', () => {
    const block = getBlock('.inline-step .step-context');
    expect(block).toContain('white-space: nowrap');
    expect(block).toContain('flex-shrink: 0');
  });

  describe('the compact counter is width-gated, not device-gated', () => {
    const row = cssRules(css).find(r => r.selector === '.inline-step' && !r.atRules);
    const containerName = row?.props.get('container-name');
    const gate = `@container ${containerName} (max-width: 26rem)`;
    const inGate = (className: string) =>
      rulesTargeting(css, className).filter(r => r.atRules === gate);
    const atTopLevel = (className: string) =>
      rulesTargeting(css, className).filter(r => !r.atRules);

    it('the row itself is the query container, and it is named', () => {
      expect(row?.props.get('container-type')).toBe('inline-size');
      expect(containerName).toBeTruthy();
    });

    it('gates on the container, in rem so it tracks the user UI scale', () => {
      expect(inGate('step-context-compact')).toHaveLength(1);
      expect(gate).toMatch(/^@container [\w-]+ \(max-width: [\d.]+rem\)$/);
    });

    it('shows the full form by default and swaps to the compact one below the gate', () => {
      expect(atTopLevel('step-context-compact').map(r => r.props.get('display'))).toEqual(['none']);
      expect(inGate('step-context-compact').map(r => r.props.get('display'))).toEqual(['inline']);
      expect(atTopLevel('step-context-full')).toEqual([]);
      expect(inGate('step-context-full').map(r => r.props.get('display'))).toEqual(['none']);
    });
  });

  describe('the mark column, held open even by a row with no mark', () => {
    const iconRules = rulesTargeting(css, 'step-icon');

    it('the slot is a fixed box, sized against the row and not the root', () => {
      const widths = iconRules.map(r => r.props.get('width')).filter(Boolean);
      expect(widths).toHaveLength(1);
      const named = /^var\((--[\w-]+)\)$/.exec(widths[0] ?? '')?.[1];
      const declared = named
        ? cssRules(css).map(r => r.props.get(named)).filter(Boolean)
        : [widths[0]];
      expect(declared).toHaveLength(1);
      expect(declared[0]).toMatch(/^\d*\.?\d+em$/);
      expect(iconRules.some(r => r.props.has('min-width'))).toBe(false);
    });

    it('nothing anywhere collapses the slot', () => {
      expect(iconRules.filter(r => r.props.get('display') === 'none')).toEqual([]);
    });
  });

  it('unfinished step is muted and struck, never the red failure treatment', () => {
    const description = getBlock('.inline-step.unfinished .step-description');
    expect(description).toContain('text-decoration: line-through');
    expect(description).toContain('color: var(--text-muted)');

    const icon = getBlock('.inline-step.unfinished .step-icon');
    expect(icon).toContain('color: var(--text-muted)');
    expect(icon).not.toContain('--accent-red');
  });

  it('unfinished step stays muted on hover', () => {
    const block = getBlock('.inline-step.unfinished .step-main:hover .step-description');
    expect(block).toContain('color: var(--text-secondary)');
  });

  it('step-main is a flex row that absorbs the width the counter will not', () => {
    const block = getBlock('.inline-step .step-main');
    expect(block).toContain('display: flex');
    expect(block).toContain('align-items: baseline');
    expect(block).toContain('flex: 1 1 auto');
    expect(block).toContain('min-width: 0');
  });

  it('both click targets strip the native button chrome, each in its own rule', () => {
    for (const selector of ['.inline-step .step-main', '.inline-step .step-context']) {
      const block = getBlock(selector);
      expect(block).toContain('background: none');
      expect(block).toContain('border: none');
      expect(block).toContain('padding: 0');
    }
  });

  it('only the button counter is interactive, and the qualified rule carries nothing else', () => {
    const interactive = getBlock('.inline-step button.step-context');
    expect(interactive).toContain('cursor: pointer');
    for (const stolen of ['color:', 'font:', 'font-size:', 'margin', 'background', 'padding']) {
      expect(interactive).not.toContain(stolen);
    }
    expect(getBlock('.inline-step .step-context')).not.toContain('cursor: pointer');
  });
});
