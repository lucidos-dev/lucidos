import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, selectorList, styleSheetPaths, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles: string = resolve(here, '..');
const repo: string = resolve(here, '../../../../..');
const read = (path: string): string => readFileSync(path, 'utf-8');

interface PartProperty { name: string; token: string; default: string }
interface Part { id: string; selector: string; parent?: string; frames: boolean; properties: PartProperty[] }
interface Catalog {
  properties: Record<string, { effect?: boolean }>;
  parts: Part[];
}
const catalog: Catalog = JSON.parse(read(resolve(repo, 'crates/lucidos-core/src/core/themes/theme-parts.json')));

interface Sheet { file: string; rules: CssRule[] }

const shellSheets: Sheet[] = styleSheetPaths(styles)
  .filter(path => !/shared-components\.css$|\/generated\//.test(path))
  .map(path => ({ file: relative(styles, path), rules: cssRules(read(path)) }));
const sharedSheet: Sheet = {
  file: 'global/shared-components.css',
  rules: cssRules(read(resolve(styles, 'global/shared-components.css'))),
};
const frameSheet: Sheet = {
  file: 'sdk_iframe.css',
  rules: [
    ...cssRules(read(resolve(repo, 'crates/lucidos-agents-api/src/api/sdk_iframe.css'))),
    ...cssRules(read(resolve(repo, 'crates/lucidos-agents-api/src/api/sdk_iframe_control_theme_parts.css'))),
  ],
};

const PART_REF = /var\(\s*(--part-[a-z0-9-]+)/g;
const tokens = new Map(catalog.parts.flatMap(part => part.properties.map(p => [p.token, part] as const)));

function readers(sheets: Sheet[]): { file: string; rule: CssRule; prop: string; value: string; token: string }[] {
  return sheets.flatMap(({ file, rules }) => rules.flatMap(rule => [...rule.props].flatMap(([prop, value]) =>
    [...value.matchAll(PART_REF)].map(m => ({ file, rule, prop, value, token: m[1] })),
  )));
}

const sameList = (a: string, b: string): boolean =>
  JSON.stringify(selectorList(a)) === JSON.stringify(selectorList(b));

describe('theme part consumers', () => {
  for (const part of catalog.parts) {
    const sheets = part.frames ? [frameSheet] : shellSheets;
    for (const property of part.properties) {
      it(`${part.id} ${property.name} is painted once, with its default as the fallback`, () => {
        const own = readers(sheets).filter(r =>
          r.token === property.token && r.prop === property.name && sameList(r.rule.selector, part.selector));
        expect(own.map(r => `${r.file}: ${r.value}`)).toEqual([
          expect.stringContaining(`var(${property.token}, ${property.default})`),
        ]);
        expect(own[0].rule.atRules).toBe('');
        expect(own[0].value).toBe(`var(${property.token}, ${property.default})`);
      });
    }
  }

  it('reads only catalog part tokens, inside the part that owns them', () => {
    for (const r of readers([...shellSheets, sharedSheet, frameSheet])) {
      const part = tokens.get(r.token);
      expect(part, `${r.file} reads ${r.token}, which is not a part token`).toBeDefined();
      if (!part) continue;
      if (catalog.parts.some(child => child.parent === part.id && sameList(r.rule.selector, child.selector))) continue;
      const scopes = selectorList(part.selector);
      const grouped = `:is(${scopes.join(', ')}) `;
      for (const selector of selectorList(r.rule.selector)) {
        const inside = selector.startsWith(grouped)
          || scopes.some(scope => selector === scope || selector.startsWith(`${scope} `));
        expect(inside, `${r.file}: ${selector} reads ${r.token} outside ${part.id}`).toBe(true);
      }
    }
  });

  it('paints shell parts in the shell only and frame parts in frames only', () => {
    for (const r of readers([sharedSheet, frameSheet])) {
      expect(tokens.get(r.token)?.frames, `${r.file} reads the shell part token ${r.token}`).toBe(true);
    }
    for (const r of readers(shellSheets)) {
      expect(tokens.get(r.token)?.frames, `${r.file} reads the frame part token ${r.token}`).toBe(false);
    }
  });

  it('names no protected surface and no container that holds one', () => {
    const NEVER = [
      /permission/, /question/, /confirm/, /prompt-dialog/, /credential/,
      /inline-form/, /plugin-(un)?install/, /event-row/, /change-/, /surface/,
      /modal/, /thread-content/, /thread-feed/, /chat-exchange/, /initiator-body/,
      /split-button/, /action-btn/,
    ];
    const EXCLUDED_SURFACE = /^\.surface(-box)?:where\(:not\(\.protected-surface\)\)$/;
    for (const part of catalog.parts) {
      for (const selector of selectorList(part.selector)) {
        if (EXCLUDED_SURFACE.test(selector)) continue;
        for (const banned of NEVER) expect(selector, `${part.id} matches ${banned}`).not.toMatch(banned);
      }
    }
  });

  it('paints the scanlines as a background image and nothing else (ADR 0313)', () => {
    const screen = catalog.parts.find(p => p.id === 'screen');
    expect(screen).toBeDefined();
    const rules = shellSheets.flatMap(({ rules }) => rules)
      .filter(rule => selectorList(rule.selector).some(s => selectorList(screen!.selector).includes(s))
        && [...rule.props.values()].some(v => v.includes('--part-screen-')));
    expect(rules.map(r => [...r.props.keys()])).toEqual([['background-image']]);
  });

  it('holds the composer caret still under reduced motion', () => {
    const still = shellSheets.flatMap(({ rules }) => rules).filter(rule =>
      rule.selector === ':root[data-motion="reduce"] .prompt-area .prompt-row .prompt-textarea');
    expect(still.map(r => r.props.get('caret-animation'))).toEqual(['manual']);
  });

  it('keeps shadow and filter parts out of every transition', () => {
    const effects = Object.entries(catalog.properties).filter(([, p]) => p.effect).map(([name]) => name);
    for (const part of catalog.parts) {
      const scopes = selectorList(part.selector);
      const sheets = part.frames ? [frameSheet, sharedSheet] : [...shellSheets, sharedSheet];
      for (const { file, rules } of sheets) {
        for (const rule of rules) {
          if (!selectorList(rule.selector).some(s => scopes.includes(s))) continue;
          const transition = `${rule.props.get('transition') ?? ''} ${rule.props.get('transition-property') ?? ''}`;
          for (const name of [...effects, 'all']) {
            expect(transition, `${file}: ${rule.selector} transitions ${name}`).not.toMatch(new RegExp(`(^|[\\s,])${name}\\b`));
          }
        }
      }
    }
  });

  it('leaves no rule painting the --text-glow alias, so no glow is applied twice', () => {
    for (const { file, rules } of [...shellSheets, sharedSheet, frameSheet]) {
      for (const rule of rules) {
        for (const value of rule.props.values()) {
          expect(value, `${file}: ${rule.selector}`).not.toContain('var(--text-glow');
        }
      }
    }
  });

  it('loads the generated @property rules and resets in the shell and in app frames', () => {
    expect(read(resolve(styles, 'global.css'))).toContain("@import './generated/theme-parts.css';");
    expect(read(resolve(repo, 'crates/lucidos-agents-api/src/api/sdk.rs')))
      .toContain('parts::codegen::frame_css()');
  });
});
