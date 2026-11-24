import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const PREVIEWS: string = readFileSync(resolve(here, '../panels/previews.css'), 'utf8');

function soleRule(className: string) {
  const rules = rulesTargeting(PREVIEWS, className);
  expect(rules, `${className} must be declared exactly once`).toHaveLength(1);
  return rules[0];
}

describe('the file preview column', () => {
  it('is the pane height, so the preview under it still resolves 100% to the pane', () => {
    expect(soleRule('file-preview-frame').props.get('height')).toBe('100%');
  });

  it('gives the preview the leftover height, and lets it shrink to it', () => {
    const body = soleRule('file-preview-frame-body').props;
    expect(body.get('flex')).toBe('1');
    expect(body.get('min-height')).toBe('0');
  });

  it('holds the path row at its own height rather than letting it flex', () => {
    expect(soleRule('file-preview-path').props.get('flex')).toBe('0 0 auto');
  });
});

describe('the path row', () => {
  it('can still break a folder or name wider than the whole row', () => {
    expect(soleRule('file-preview-path').props.get('overflow-wrap')).toBe('anywhere');
  });

  it('wraps between crumbs, never between a folder and its separator', () => {
    expect(soleRule('file-preview-path-crumb').props.get('white-space')).toBe('nowrap');
  });

  it('moves the file name to the next line whole, rather than breaking it at a hyphen', () => {
    const name = soleRule('file-preview-path-name').props;
    expect(name.get('display')).toBe('inline-block');
    expect(name.get('max-width')).toBe('100%');
  });

  it('takes the content pane structural gutter from the spacing scale', () => {
    expect(soleRule('file-preview-path').props.get('padding')).toBe('var(--space-sm) var(--space-lg)');
  });
});
