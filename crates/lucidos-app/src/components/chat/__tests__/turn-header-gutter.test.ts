import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const inputCss = readFileSync(resolve(here, '../../../styles/chat/input-messages.css'), 'utf-8');
const responseCss = readFileSync(resolve(here, '../../../styles/chat/response.css'), 'utf-8');

function getBlock(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const re = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'g');
  return [...css.matchAll(re)].map(m => m[1]).join('\n');
}

function declarationValue(block: string, property: string): string | undefined {
  const escaped = property.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return block.match(new RegExp(`${escaped}\\s*:\\s*([^;]+)`))?.[1].trim();
}

describe('turn header gutter', () => {
  it('keeps actor and executor icons aligned with turn body content', () => {
    const initiatorHeader = getBlock(inputCss, '.initiator-header');
    const initiatorBody = getBlock(inputCss, '.initiator-body');
    const responseHeader = getBlock(responseCss, '.response-header');
    const responseContent = getBlock(responseCss, '.response-content');

    expect(declarationValue(initiatorHeader, 'padding-left')).toBe('var(--turn-body-inset)');
    expect(declarationValue(responseHeader, 'padding-left')).toBe('var(--turn-body-inset)');
    expect(declarationValue(initiatorBody, 'padding-left')).toBe('var(--turn-body-inset)');
    expect(declarationValue(responseContent, 'padding-left')).toBe('var(--turn-body-inset)');
  });

  it('insets the collapsed marker to match the turn body content', () => {
    const turnCollapsed = getBlock(inputCss, '.turn-collapsed');
    expect(declarationValue(turnCollapsed, 'padding-left')).toBe('var(--turn-body-inset)');
  });
});
