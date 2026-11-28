import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import postcss from 'postcss';
import ts from 'typescript';
import { clientSourcePaths, cssRules, selectorList, styleSheetPaths } from './css-rule-helpers';

const SRC = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const TONE_COMPONENT = 'components/shared/ToggleIconButton.tsx';

const TONE_SHEETS = ['styles/global/host-components.css', 'styles/panels/shell.css'];

const TOGGLE_CLASSES = ['icon-btn', 'pin-btn'];

const PRESSED_CHOICES = ['question-option', 'pill-bar-btn', 'segmented-btn', '[data-role="call-toggle"]', 'terminal-key'];

const RETIRED_ON_CLASSES = /\.(?:filter-active|view-selector-active)(?![\w-])/;

const PRESSED = /\[aria-pressed(?:=["']?true["']?)?\]/;

const ACTIVE = /\.active(?![\w-])/;

function relativeToSrc(file: string): string {
  return relative(SRC, file);
}

function handPressedIconButtons(source: string, fileName = 'fixture.tsx'): string[] {
  const ast = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const found: string[] = [];
  const visit = (node: ts.Node): void => {
    if ((ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) && node.tagName.getText() === 'button') {
      const attrs = node.attributes.properties.filter(ts.isJsxAttribute);
      const named = (name: string) => attrs.find(a => a.name.getText() === name);
      const classText = (named('class') ?? named('className'))?.initializer?.getText() ?? '';
      if (named('aria-pressed') && /\bicon-btn\b/.test(classText)) {
        found.push(`${fileName}:${ast.getLineAndCharacterOfPosition(node.getStart()).line + 1}`);
      }
    }
    node.forEachChild(visit);
  };
  visit(ast);
  return found;
}

function stray(css: string, sheet: string): string[] {
  const inToneSheet = TONE_SHEETS.includes(sheet);
  const out: string[] = [];
  for (const rule of cssRules(css)) {
    for (const member of selectorList(rule.selector)) {
      if (RETIRED_ON_CLASSES.test(member)) {
        out.push(member);
        continue;
      }
      const compounds = postcss.list.space(member.replace(/\s*[>+~]\s*/g, ' '));
      const paintsToggleOn = compounds.some(
        c => (PRESSED.test(c) && !PRESSED_CHOICES.some(choice => member.includes(choice)))
          || (ACTIVE.test(c) && TOGGLE_CLASSES.some(cls => c.includes(`.${cls}`))),
      );
      if (!paintsToggleOn) continue;
      if (inToneSheet && (member.includes('[data-tone') || member.includes('.crossfade-glyph'))) continue;
      out.push(member);
    }
  }
  return out;
}

describe('an icon toggle has one definition', () => {
  it('no icon-btn button writes aria-pressed by hand', () => {
    const offenders = clientSourcePaths(SRC)
      .filter(file => relativeToSrc(file) !== TONE_COMPONENT)
      .flatMap(file => handPressedIconButtons(readFileSync(file, 'utf8'), relativeToSrc(file)));
    expect(offenders, 'render the toggle through ToggleIconButton').toEqual([]);
  }, 30_000);

  it('flags a hand-pressed icon button (fixture)', () => {
    const source = `const a = <button class={\`icon-btn header-icon\`} aria-pressed={on} />;
      const b = <button class="pill-bar-btn" aria-pressed={on} />;
      const c = <button class="icon-btn" onClick={f} />;`;
    expect(handPressedIconButtons(source)).toEqual(['fixture.tsx:1']);
  });

  it('no sheet paints an icon toggle on-state outside the tone rules', () => {
    const offenders = styleSheetPaths(SRC).flatMap(file => {
      const sheet = relativeToSrc(file);
      return stray(readFileSync(file, 'utf8'), sheet).map(member => `${sheet}: ${member}`);
    });
    expect(offenders, 'a new look is a new tone in ToggleIconButton.tsx, never a local rule').toEqual([]);
  });

  it('flags a local on-state rule (fixture)', () => {
    const css = `
      .pin-btn.active { color: red; }
      .icon-btn[aria-pressed="true"] { color: red; }
      .toolbar .icon-btn.active:hover { color: red; }
      .filter-active { color: red; }
      .icon-btn[aria-pressed="false"]:hover { color: red; }
      .pill-bar-btn.active { color: red; }
      .dashboard-panel-toggle[aria-pressed="true"] { color: red; }
      .question-option[aria-pressed="true"] { color: red; }
      .icon-btn[aria-pressed="true"][data-tone="framed"] { color: red; }`;
    expect(stray(css, 'styles/other.css')).toEqual([
      '.pin-btn.active',
      '.icon-btn[aria-pressed="true"]',
      '.toolbar .icon-btn.active:hover',
      '.filter-active',
      '.dashboard-panel-toggle[aria-pressed="true"]',
      '.icon-btn[aria-pressed="true"][data-tone="framed"]',
    ]);
    expect(stray(css, 'styles/global/host-components.css')).not.toContain(
      '.icon-btn[aria-pressed="true"][data-tone="framed"]',
    );
  });
});
