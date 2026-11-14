import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { resolve } from 'node:path';
import { FONT_CATALOG } from '../../../../../packages/lucidos-sdk/src/generated/font-catalog';
import { block, cssRules, decl, REPO_ROOT } from './css-rule-helpers';

const base = readFileSync(resolve(REPO_ROOT, 'crates/lucidos-app/src/styles/global/base.css'), 'utf8');
const iframe = readFileSync(resolve(REPO_ROOT, 'crates/lucidos-engine/src/api/sdk_iframe.css'), 'utf8');

describe('native monospace font stack', () => {
  it('keeps protected, bootstrap and theme defaults aligned with the catalog', () => {
    const stack = FONT_CATALOG.find(font => font.id === 'monospace')!.stack;
    const protectedRules = cssRules(base).filter(rule => rule.props.has('--protected-font-mono'));
    expect(protectedRules).toHaveLength(2);
    for (const rule of protectedRules) expect(rule.props.get('--protected-font-mono')).toBe(stack);
    const bootstrap = readFileSync(resolve(REPO_ROOT, 'crates/lucidos-app/index.html'), 'utf8');
    expect(bootstrap).toContain(`font-family: ${stack};`);
    const catalog = JSON.parse(readFileSync(
      resolve(REPO_ROOT, 'crates/lucidos-engine/src/core/themes/theme-tokens.json'), 'utf8',
    )) as { tokens: { name: string; default: { dark: string; light: string } }[] };
    expect(catalog.tokens.find(token => token.name === '--font-mono')!.default).toEqual({
      dark: stack, light: stack,
    });
  });
  it('prefers OS fonts over user-installed coding fonts', () => {
    expect(decl(block(base, ':root'), '--font-mono')).toBe(
      FONT_CATALOG.find(font => font.id === 'monospace')!.stack,
    );
  });

  it('keeps the host and SDK iframe defaults identical', () => {
    const hostStack = decl(block(base, ':root'), '--font-mono');
    expect(hostStack).not.toBeNull();
    expect(decl(block(iframe, ':root'), '--font-mono')).toBe(hostStack);
  });
});
