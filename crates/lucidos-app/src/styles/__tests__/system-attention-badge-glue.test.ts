import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { block, decl } from './css-rule-helpers';

const CSS = readFileSync(
  fileURLToPath(new URL('../global/host-components.css', import.meta.url)), 'utf8',
);
const MARKUP = readFileSync(
  fileURLToPath(new URL('../../components/shared/SystemAttentionBadge.tsx', import.meta.url)), 'utf8',
);

describe('the What\'s New badge dot', () => {
  it('wears the accent token, never a colour of its own', () => {
    expect(decl(block(CSS, '.system-attention-badge {'), 'background')).toBe('var(--accent)');
  });

  it('takes its corner placement from `.badge`, never a second copy', () => {
    const corner = block(CSS, '.system-attention-badge-corner {');
    expect(decl(corner, 'position')).toBe(null);
    expect(decl(corner, 'background')).toBe(null);
    expect(decl(corner, 'padding')).toBe('0');
    expect(decl(corner, 'border-radius')).toBe(null);
  });

  it('outranks `.badge`, which a later sheet would otherwise win on order', () => {
    expect(CSS).toMatch(/\n\.badge\.system-attention-badge-corner \{/);
  });
});

describe('the inline slot', () => {
  const slot = block(CSS, '.system-attention-badge-slot {');

  it('is a plain inline box, so the joiner is not hidden by an atomic one', () => {
    expect(decl(slot, 'display')).toBe('inline');
    expect(decl(slot, 'line-height')).toBe('0');
  });

  it('leaves the explainer\'s own slot rule alone', () => {
    expect(CSS).toMatch(/\n\.explainer-slot \{/);
  });

  it('forbids a break either side of the dot', () => {
    expect(decl(block(CSS, '.system-attention-badge-slot::before {'), 'content')).toBe("'\\2060'");
  });

  it('keeps the joiner out of the markup, so it stays out of textContent', () => {
    expect(MARKUP).not.toContain('⁠');
  });
});
