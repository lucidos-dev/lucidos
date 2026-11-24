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

describe('nav focus marker — uniform gap outside chat', () => {
  it('gives a focus-marked settings row a uniform gap that fits its clip container', () => {
    const block = getBlock(settingsCss, '.settings-row.nav-focus-stuck');
    expect(block).not.toBe('');
    expect(declarationValue(block, 'padding')).toBe('0.375rem');
    expect(declarationValue(block, 'margin')).toBe('0.125rem -0.375rem');
    const section = getBlock(settingsCss, '.settings-section');
    expect(declarationValue(section, 'display')).toBe('flow-root');
  });

  it('preserves a nested settings row indent under the marker', () => {
    const block = getBlock(settingsCss, '.settings-row-child.nav-focus-stuck');
    expect(block).not.toBe('');
    expect(declarationValue(block, 'margin-left')).toBe('0.625rem');
  });

  it('shrinks a focus-marked plugin row inward to a uniform gap', () => {
    const block = getBlock(pagesCss, '.app-store-plugin-row.nav-focus-stuck');
    expect(block).not.toBe('');
    expect(declarationValue(block, 'padding')).toBe('var(--space-md)');
    expect(declarationValue(block, 'margin')).toBe('var(--space-xs)');
    const row = getBlock(pagesCss, '.list-row.app-store-plugin-row');
    expect(declarationValue(row, 'padding-top')).toBe('var(--space-lg)');
    expect(declarationValue(row, 'padding-bottom')).toBe('var(--space-lg)');
  });

  it('has no pre-rename marker class names left behind', () => {
    expect(settingsCss).not.toMatch(/\.event-pulse|\.event-focus-stuck/);
    expect(pagesCss).not.toMatch(/\.event-pulse|\.event-focus-stuck/);
  });
});
