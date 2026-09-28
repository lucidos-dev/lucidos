/**
 * Protected surfaces: no theme and no style override can make an approval
 * unreadable or misleading (ADR 0309).
 *
 * The engine derives and clamps the `--protected-*` palette
 * (`core/themes/protected.rs`, with its own tests). This suite pins the shell's
 * half of the promise:
 *
 *   - `.protected-surface` maps every catalog paint token to that palette, so
 *     no theme value reaches inside;
 *   - every protected surface carries the class;
 *   - the CSS those surfaces use reads its colours only through mapped tokens;
 *   - `base.css` carries a literal default for every protected token.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { clientSourcePaths, cssRules } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const src: string = resolve(here, '../..');
const read = (path: string): string => readFileSync(resolve(src, path), 'utf-8');

const protectedCss = read('styles/global/protected-surface.css');
const baseCss = read('styles/global/base.css');
const catalog: { tokens: { name: string; kind: string }[] } = JSON.parse(
  read('../../lucidos-engine/src/core/themes/theme-tokens.json'),
);

const PROTECTED_CLASS = 'protected-surface';

/** Catalog kinds that cannot hide or recolour content, so the map may leave
 *  them to the theme. A radius rounds a corner, and every surface pads its
 *  content away from its corners. A new kind is not in this list, so the guard
 *  fails until someone decides. */
const HARMLESS_KINDS = new Set(['length']);

function declarations(css: string, selector: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const rule of cssRules(css)) {
    if (rule.selector !== selector || rule.atRules !== '') continue;
    for (const [prop, value] of rule.props) out.set(prop, value);
  }
  return out;
}

const remap = declarations(protectedCss, '.protected-surface');
const varNames = (value: string): string[] =>
  [...value.matchAll(/var\(\s*(--[a-z0-9-]+)/g)].map(m => m[1]);

describe('the .protected-surface map', () => {
  it('isolates the surface into its own stacking context', () => {
    expect(remap.get('isolation')).toBe('isolate');
  });

  it('maps every catalog paint token, so no theme value reaches inside', () => {
    const missing = catalog.tokens
      .filter(t => !HARMLESS_KINDS.has(t.kind) && !remap.has(t.name))
      .map(t => `${t.name} (${t.kind})`);
    expect(missing, 'map these in protected-surface.css').toEqual([]);
  });

  it('maps each token to the protected palette or a literal, never to another theme token', () => {
    for (const [name, value] of remap) {
      if (!name.startsWith('--')) continue;
      for (const ref of varNames(value)) {
        expect(ref.startsWith('--protected-'), `${name}: ${value}`).toBe(true);
      }
    }
  });

  it('pins the type and spacing scales at their base.css values', () => {
    const root = declarations(baseCss, ':root');
    const scale = [...root.keys()].filter(n => /^--(font-size|space)-/.test(n));
    expect(scale.length).toBeGreaterThan(0);
    for (const name of scale) expect(remap.get(name), name).toBe(root.get(name));
  });
});

describe('the protected palette defaults', () => {
  const used = new Set([...protectedCss.matchAll(/var\(\s*(--protected-[a-z0-9-]+)/g)].map(m => m[1]));
  const dark = declarations(baseCss, 'html, html[data-theme-mode="dark"]');
  const light = declarations(baseCss, 'html[data-theme-mode="light"]');

  it('are declared in both theme blocks as literals, so no override flows through', () => {
    expect(used.size).toBeGreaterThan(0);
    for (const name of used) {
      for (const [mode, block] of [['dark', dark], ['light', light]] as const) {
        const value = block.get(name);
        expect(value, `${name} has no ${mode} default in base.css`).toBeDefined();
        expect(value, `${name} (${mode})`).not.toContain('var(');
      }
    }
  });
});

/** Root classes only a protected surface wears. Every class list naming one,
 *  in any component, must also name `protected-surface`. */
const APPROVAL_ROOTS = [
  'permission-body',
  'question-body',
  'confirm-dialog',
  'split-button-primary',
  'split-button-caret',
  'split-button-menu',
];

/** Surfaces whose root class is shared with ordinary views, so they are named
 *  by file. */
const SURFACES: { file: string; root: string }[] = [
  { file: 'components/credentials/CredentialModal.tsx', root: 'inline-form' },
  { file: 'components/plugins/PluginInstallPanel.tsx', root: 'inline-form' },
  { file: 'components/plugins/PluginUninstallPanel.tsx', root: 'inline-form' },
  { file: 'components/email/EmailConfirmModal.tsx', root: 'inline-form' },
  { file: 'components/changes/ChangesView.tsx', root: 'panel-content' },
];

function classLists(source: string): string[] {
  return [...source.matchAll(/(?:class|panelClass|overlayClass)=(?:"([^"]*)"|\{`([^`]*)`\})/g)]
    .map(m => (m[1] ?? m[2]).replace(/\$\{[^}]*\}/g, ' '));
}

describe('every protected surface carries the class', () => {
  it.each(APPROVAL_ROOTS)('every .%s in any component', root => {
    let seen = 0;
    for (const path of clientSourcePaths(resolve(src, 'components'))) {
      for (const list of classLists(readFileSync(path, 'utf-8'))) {
        if (!list.split(/\s+/).includes(root)) continue;
        seen++;
        expect(list.split(/\s+/), `${path}: ${list}`).toContain(PROTECTED_CLASS);
      }
    }
    expect(seen, `no component names .${root}`).toBeGreaterThan(0);
  });

  it.each(SURFACES)('$file: .$root', ({ file, root }) => {
    const lists = classLists(read(file)).filter(list => list.split(/\s+/).includes(root));
    expect(lists.length, `no class list names ${root}`).toBeGreaterThan(0);
    for (const list of lists) expect(list.split(/\s+/), list).toContain(PROTECTED_CLASS);
  });

  it.each([
    'components/shared/ConfirmDialog.tsx',
    'components/shared/PromptDialog.tsx',
    'components/shared/ProgressDialog.tsx',
  ])(
    '%s puts its backdrop inside it too, so the scrim is protected',
    file => expect(read(file)).toContain(`overlayClass="${PROTECTED_CLASS}"`),
  );

  it('builds every change-action button on the banner, and each folded row, as one', () => {
    const banner = read('components/chat/WaitingBanner.tsx');
    expect(banner).toMatch(/function protectedButtonClass[\s\S]*PROTECTED_SURFACE/);
    expect(banner.match(/extraClass: PROTECTED_SURFACE/g)?.length).toBe(2);
    expect(banner).toContain('class={`thread-overflow-item ${PROTECTED_SURFACE}`}');
    expect(banner).not.toMatch(/class=\{?['"`]action-btn action-btn-(confirm|danger)['"`]/);
  });

  it('carries the class onto a popover portalled out of a protected surface', () => {
    expect(read('components/shared/Dropdown.tsx')).toContain('protectedClassFrom(trigger)');
    expect(read('components/shared/Explainer.tsx')).toContain('protectedClassFrom(anchor)');
  });
});

/** Stylesheets that style the surfaces above, and the selectors that mark a
 *  rule as theirs. */
const SURFACE_SHEETS = [
  'styles/chat/response.css',
  'styles/pages.css',
  'styles/components.css',
  'styles/global/host-components.css',
  'styles/global/shared-components.css',
  'styles/global/modal-overlay.css',
  'styles/global/surface.css',
];
const SURFACE_SELECTOR =
  /\.(permission-|question-|credential-|plugin-install|confirm-|email-confirm|split-button|changes-|action-btn|btn-save|btn-cancel|inline-form|modal-overlay|surface\b)/;
const COLOUR_PROPS =
  /^(color|background(-color|-image)?|border(-(top|right|bottom|left))?(-color)?|outline(-color)?|box-shadow|text-shadow|fill|stroke|caret-color|accent-color|text-decoration-color)$/;

describe('the CSS a protected surface uses', () => {
  it('reads colour only through tokens the map covers', () => {
    const local = new Set<string>();
    const reads: { where: string; name: string }[] = [];
    for (const sheet of SURFACE_SHEETS) {
      for (const rule of cssRules(read(sheet))) {
        if (!SURFACE_SELECTOR.test(rule.selector)) continue;
        for (const [prop, value] of rule.props) {
          // A local token is followed through its own value, like a colour.
          if (prop.startsWith('--')) local.add(prop);
          if (!prop.startsWith('--') && !COLOUR_PROPS.test(prop)) continue;
          for (const name of varNames(value)) reads.push({ where: `${sheet} ${rule.selector} { ${prop} }`, name });
        }
      }
    }
    // A part token needs no remap: the generated reset sets every one to
    // initial on the surface, which the next block pins.
    const unmapped = reads
      .filter(({ name }) => !name.startsWith('--protected-') && !name.startsWith('--part-')
        && !remap.has(name) && !local.has(name))
      .map(({ where, name }) => `${where}: ${name}`);
    expect(unmapped, 'map these tokens on .protected-surface').toEqual([]);
  });

  it('paints no literal colour, which the clamp could not see', () => {
    const literal = /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|oklch|oklab)\(/i;
    // A hover rule that protected-surface.css restates for the same classes
    // is overridden inside every protected surface.
    const classesOf = (selector: string) =>
      new Set([...selector.matchAll(/\.([a-z0-9-]+)/g)].map(m => m[1]).filter(c => c !== PROTECTED_CLASS));
    const restatedHover = cssRules(protectedCss)
      .filter(r => r.selector.includes(':hover'))
      .flatMap(r => r.selector.split(',').map(classesOf));
    const restated = (selector: string) =>
      selector.includes(':hover')
      && restatedHover.some(covered => [...classesOf(selector)].every(c => covered.has(c)));
    const found: string[] = [];
    for (const sheet of SURFACE_SHEETS) {
      for (const rule of cssRules(read(sheet))) {
        if (!SURFACE_SELECTOR.test(rule.selector)) continue;
        if (rule.selector.split(',').every(sel => restated(sel))) continue;
        for (const [prop, value] of rule.props) {
          if (!COLOUR_PROPS.test(prop) || prop.includes('shadow')) continue;
          if (literal.test(value)) found.push(`${sheet} ${rule.selector} { ${prop}: ${value} }`);
        }
      }
    }
    expect(found).toEqual([]);
  });
});

describe('theme parts never reach a protected surface (ADR 0307)', () => {
  const partsCatalog: {
    properties: Record<string, { inherits: boolean }>;
    parts: { properties: { token: string }[] }[];
  } = JSON.parse(read('../../lucidos-engine/src/core/themes/theme-parts.json'));
  const generated = read('styles/generated/theme-parts.css');

  it('resets every part token on the surface', () => {
    const reset = declarations(generated, '.protected-surface');
    for (const part of partsCatalog.parts) {
      for (const { token } of part.properties) expect(reset.get(token), token).toBe('initial');
    }
  });

  it('resets every inherited part property, so none flows in from a container', () => {
    const reset = declarations(generated, ':where(.protected-surface)');
    const inherited = Object.entries(partsCatalog.properties).filter(([, p]) => p.inherits).map(([n]) => n);
    expect(inherited.length).toBeGreaterThan(0);
    for (const name of inherited) expect(reset.has(name), name).toBe(true);
  });

  it('paints an inline surface above its neighbours', () => {
    const stack = declarations(protectedCss, ':where(.protected-surface:not(.modal-overlay))');
    expect(stack.get('position')).toBe('relative');
    expect(stack.get('z-index')).toBe('1');
  });
});
