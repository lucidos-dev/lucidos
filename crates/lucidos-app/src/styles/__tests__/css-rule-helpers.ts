import { expect } from 'vitest';
import postcss, { type AtRule, type Container, type Declaration, type Document } from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

export const REPO_ROOT: string = resolve(dirname(fileURLToPath(import.meta.url)), '../../../../..');

export const ENGINE_SERVED_CSS = [
  'crates/lucidos-agents-api/src/api/sdk_iframe.css',
  'crates/lucidos-agents-api/src/api/sdk_iframe_shapes.css',
  'crates/lucidos-agents-api/src/api/sdk_iframe_control_theme_parts.css',
  'crates/lucidos-app/src/styles/generated/theme-parts-frame.css',
] as const;

export function styleSheetPaths(root: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(root)) {
    const path: string = resolve(root, entry);
    if (statSync(path).isDirectory()) {
      if (entry === '__tests__') continue;
      out.push(...styleSheetPaths(path));
    } else if (path.endsWith('.css')) {
      out.push(path);
    }
  }
  return out;
}

export function clientSourcePaths(root: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(root)) {
    const path: string = resolve(root, entry);
    if (statSync(path).isDirectory()) {
      if (entry === '__tests__' || entry === 'generated') continue;
      out.push(...clientSourcePaths(path));
    } else if (/\.tsx?$/.test(entry) && !/\.test\.tsx?$/.test(entry)) {
      out.push(path);
    }
  }
  return out;
}

export function block(css: string, needle: string, from = 0): string {
  const at = css.indexOf(needle, from);
  expect(at, `"${needle}" not found`).toBeGreaterThanOrEqual(0);
  const open = css.indexOf('{', at);
  let depth = 0;
  for (let i = open; i < css.length; i++) {
    if (css[i] === '{') depth++;
    else if (css[i] === '}' && --depth === 0) return css.slice(open + 1, i);
  }
  throw new Error(`unterminated block for "${needle}"`);
}

export function decl(body: string, prop: string): string | null {
  const m = body.match(new RegExp(`(?:^|[;{]|\\*/)\\s*${prop}:\\s*([^;]+);`));
  return m ? m[1].replace(/\s+/g, ' ').trim() : null;
}

export function focusRingWidthRem(baseCss: string): number {
  const m = /--focus-ring-width:\s*([\d.]+)rem;/.exec(baseCss);
  expect(m, '--focus-ring-width is no longer a rem token in base.css').not.toBeNull();
  return parseFloat(m![1]);
}

export interface CssRule {
  selector: string;
  atRules: string;
  body: string;
  props: Map<string, string>;
}

const parsedSheets = new Map<string, CssRule[]>();

export function cssRules(css: string): CssRule[] {
  let rules = parsedSheets.get(css);
  if (!rules) {
    rules = parseRules(css);
    parsedSheets.set(css, rules);
  }
  return [...rules];
}

function parseRules(css: string): CssRule[] {
  const out: CssRule[] = [];
  postcss.parse(css).walkRules(rule => {
    const at: string[] = [];
    for (let node: Container | Document | undefined = rule.parent; node; node = node.parent) {
      if (node.type === 'atrule') {
        const a = node as AtRule;
        at.unshift(`@${a.name} ${a.params}`.trim());
      }
    }
    const decls = rule.nodes.filter((n): n is Declaration => n.type === 'decl');
    out.push({
      selector: rule.selector.replace(/\s+/g, ' '),
      atRules: at.join(' '),
      body: decls.map(d => `${d.prop}: ${d.value}`).join('; '),
      props: new Map(decls.map(d => [d.prop, d.value.replace(/\s+/g, ' ').trim()])),
    });
  });
  return out;
}

export const REDUCED_MOTION_ROOT = ':root[data-motion="reduce"]';

export function isReducedMotionRule(rule: CssRule): boolean {
  return selectorList(rule.selector).every(s => s.startsWith(REDUCED_MOTION_ROOT));
}

export const COLUMN_VIEW_ROOTS = [
  'content-view', 'panel-content', 'settings-panel', 'apps-view', 'plugins-view',
  'notification-detail', 'plugin-detail', 'inline-form',
] as const;

export const CONTENT_VIEW_ROOTS = [
  ...COLUMN_VIEW_ROOTS, 'file-preview-frame', 'app-ui-inline', 'url-preview-inline',
] as const;

export function selectorList(selector: string): string[] {
  return postcss.list.comma(selector).map(s => s.trim());
}

function flattenCombinators(selector: string): string {
  let depth = 0;
  let out = '';
  for (const ch of selector) {
    if (ch === '(' || ch === '[') depth++;
    else if (ch === ')' || ch === ']') depth--;
    out += depth === 0 && (ch === '>' || ch === '+' || ch === '~') ? ' ' : ch;
  }
  return out;
}

function stripHas(selector: string): string {
  let out = '';
  for (let i = 0; i < selector.length; i++) {
    if (!selector.startsWith(':has(', i)) { out += selector[i]; continue; }
    let depth = 0;
    for (i += 4; i < selector.length; i++) {
      if (selector[i] === '(') depth++;
      else if (selector[i] === ')' && --depth === 0) break;
    }
  }
  return out;
}

export function rulesTargeting(css: string, className: string): CssRule[] {
  const token = new RegExp(`\\.${className.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?![\\w-])`);
  return cssRules(css).filter(rule =>
    postcss.list.comma(rule.selector).some(one => {
      const compounds = postcss.list.space(flattenCombinators(stripHas(one)));
      const subject = compounds[compounds.length - 1] ?? '';
      return !subject.includes('::') && token.test(subject);
    }),
  );
}
