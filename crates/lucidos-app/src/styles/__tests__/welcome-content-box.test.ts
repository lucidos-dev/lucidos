import { describe, it, expect } from 'vitest';
import postcss, { type Rule } from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const RESPONSE_CSS = resolve(here, '../chat/response.css');
const INPUT_CSS = resolve(here, '../chat/input-messages.css');

function declarationsOf(cssPath: string, selector: string): Record<string, string> {
  const root = postcss.parse(readFileSync(cssPath, 'utf8'));
  const out: Record<string, string> = {};
  root.walkRules((rule: Rule) => {
    if (rule.selector.trim() !== selector) return;
    rule.walkDecls((d) => {
      out[d.prop] = d.value.trim();
    });
  });
  return out;
}

describe('.welcome-message shares the composer content box', () => {
  it('caps, centres and insets itself', () => {
    const welcome = declarationsOf(RESPONSE_CSS, '.welcome-message');
    expect(welcome['max-width']).toBe('var(--content-max-width)');
    expect(welcome['margin-left']).toBe('auto');
    expect(welcome['margin-right']).toBe('auto');
    expect(welcome['padding-inline']).toBe('var(--turn-body-inset)');
  });

  it('uses the same cap and inset the composer column does', () => {
    const welcome = declarationsOf(RESPONSE_CSS, '.welcome-message');
    const composer = declarationsOf(INPUT_CSS, '.prompt-input-container');
    expect(welcome['max-width']).toBe(composer['max-width']);
    expect(welcome['padding-inline']).toBe(composer['padding-inline']);
  });

  it('leaves the hero variant no second copy of the box', () => {
    const hero = declarationsOf(RESPONSE_CSS, '.welcome-hero');
    expect(hero['max-width']).toBeUndefined();
    expect(hero['margin-left']).toBeUndefined();
    expect(hero['padding-inline']).toBeUndefined();
  });

  it('still declares the `.response-content` override this defends against', () => {
    const response = declarationsOf(RESPONSE_CSS, '.response-content');
    expect(response['max-width']).toBe('100%');
  });
});
