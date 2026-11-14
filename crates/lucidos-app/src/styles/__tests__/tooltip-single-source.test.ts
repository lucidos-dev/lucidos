/**
 * The Lucidos tooltip has ONE implementation, and this pins that.
 *
 * The CSS lives in `styles/global/shared-components.css`, which the engine
 * appends to the served `/api/v1/sdk-iframe.css`. The behaviour lives in
 * `packages/lucidos-sdk/src/tooltip.ts`, which the host hook calls. So the host
 * shell and every app iframe run the same tooltip.
 *
 * Three things have to stay true, and no single file shows them. The CSS lives
 * in exactly one place. The hook stays a wrapper. Every token the shared rules
 * name resolves in BOTH hosts.
 *
 * That last one is the quiet failure. An app iframe never loads `base.css`, so
 * a token defined only there paints nothing, and nothing breaks loudly.
 *
 * Why not a parity test over two copies: docs/plans/2026-08-21-sdk-iframe-tooltip.md.
 */
import { describe, it, expect } from 'vitest';
import postcss, { type Root, type Rule } from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
/** Repo root, from `crates/lucidos-app/src/styles/__tests__/`. */
const REPO_ROOT = resolve(here, '../../../../..');

const SHARED_CSS = 'crates/lucidos-app/src/styles/global/shared-components.css';
const HOST_CSS = 'crates/lucidos-app/src/styles/global/base.css';
const IFRAME_CSS = 'crates/lucidos-engine/src/api/sdk_iframe.css';
const HOOK_TS = 'crates/lucidos-app/src/hooks/useTooltip.ts';
const APP_SRC = 'crates/lucidos-app/src';

/** Host-only overrides, allowed because an app iframe never matches them. */
const ALLOWED_OVERRIDES: Record<string, string[]> = {
  'crates/lucidos-app/src/styles/global/modal-overlay.css': [':root[data-ui-blocked] #tooltip'],
};

function read(relPath: string): string {
  return readFileSync(resolve(REPO_ROOT, relPath), 'utf8');
}

function parse(relPath: string): Root {
  return postcss.parse(read(relPath), { from: resolve(REPO_ROOT, relPath) });
}

/** Every rule whose selector targets the tooltip's own DOM. */
function tooltipRules(root: Root): Rule[] {
  const rules: Rule[] = [];
  root.walkRules((rule) => {
    if (rule.selector.includes('#tooltip')) rules.push(rule);
  });
  return rules;
}

/** Every stylesheet the tooltip could be restyled from, minus the shared one. */
function everyOtherStylesheet(): string[] {
  const found = readdirSync(resolve(REPO_ROOT, APP_SRC), { recursive: true }) as string[];
  const appSheets = found
    .filter((p: string) => p.endsWith('.css'))
    .map((p: string) => `${APP_SRC}/${p}`);
  return [...appSheets, IFRAME_CSS].filter((p) => p !== SHARED_CSS);
}

/** Custom properties a stylesheet DECLARES, wherever it declares them. */
function declaredTokens(root: Root): Set<string> {
  const names = new Set<string>();
  root.walkDecls((decl) => {
    if (decl.prop.startsWith('--')) names.add(decl.prop);
  });
  return names;
}

/** Custom properties a set of rules READS, mapped to "every read has a fallback". */
function readTokens(rules: Rule[]): Map<string, boolean> {
  const reads = new Map<string, boolean>();
  for (const rule of rules) {
    rule.walkDecls((decl) => {
      for (const [, name, comma] of decl.value.matchAll(/var\(\s*(--[\w-]+)\s*(,)?/g)) {
        reads.set(name, (reads.get(name) ?? true) && Boolean(comma));
      }
    });
  }
  return reads;
}

describe('the tooltip has one source of truth', () => {
  it(`declares its rules in ${SHARED_CSS}`, () => {
    const selectors = tooltipRules(parse(SHARED_CSS)).map((r) => r.selector);
    expect(selectors).toContain('#tooltip');
    expect(selectors).toContain('#tooltip-arrow');
    expect(selectors).toContain('#tooltip.above #tooltip-arrow');
    expect(selectors).toContain('#tooltip-text');
  });

  it('declares no second copy in any other stylesheet', () => {
    const copies: string[] = [];
    for (const relPath of everyOtherStylesheet()) {
      const allowed = ALLOWED_OVERRIDES[relPath] ?? [];
      for (const rule of tooltipRules(parse(relPath))) {
        if (!allowed.includes(rule.selector)) copies.push(`${relPath}: ${rule.selector}`);
      }
    }
    expect(
      copies,
      `The tooltip is styled once, in ${SHARED_CSS}, which the host imports and the `
      + 'engine appends to /api/v1/sdk-iframe.css. A rule anywhere else reaches the '
      + 'host alone, so an app silently misses it. That is how the narrow-viewport '
      + 'cap sat in mobile.css and never reached an app.',
    ).toEqual([]);
  });

  it('names only tokens that resolve in the host AND in an app iframe', () => {
    const hostTokens = declaredTokens(parse(HOST_CSS));
    const iframeTokens = declaredTokens(parse(IFRAME_CSS));
    const unresolved: string[] = [];
    for (const [name, hasFallback] of readTokens(tooltipRules(parse(SHARED_CSS)))) {
      if (!hostTokens.has(name)) unresolved.push(`${name} (missing from ${HOST_CSS})`);
      if (!iframeTokens.has(name) && !hasFallback) {
        unresolved.push(`${name} (missing from ${IFRAME_CSS}, and no var() fallback)`);
      }
    }
    expect(
      unresolved,
      "An app iframe never loads base.css. Mirror the token into the engine's "
      + 'sdk_iframe.css, or give the read a var() fallback.',
    ).toEqual([]);
  });

  it('leaves the host hook a wrapper, with no second implementation', () => {
    const hook = read(HOOK_TS);
    expect(hook).toContain('@lucidos/tooltip');
    for (const banned of ['addEventListener', 'getBoundingClientRect', 'createElement']) {
      expect(
        hook.includes(banned),
        `${HOOK_TS} calls ${banned}, so it has grown a tooltip of its own again. The `
        + 'behaviour belongs in packages/lucidos-sdk/src/tooltip.ts, which both hosts call.',
      ).toBe(false);
    }
  });
});
