/**
 * An icon toggle has ONE definition: `ToggleIconButton` renders it, with
 * `aria-pressed` for the on-state and a `data-tone` for the look. Each tone is
 * painted once per surface (`host-components.css` for the page, `shell.css` for
 * the header bar). Two scans keep it that way.
 *
 * 1. No `<button>` that wears `icon-btn` writes `aria-pressed` by hand. That is
 *    the markup half of a second definition.
 * 2. No sheet paints a pressed state, or the `.active` of `.icon-btn` or
 *    `.pin-btn`, outside the tone rules. That is the paint half: a local `.active` or `[aria-pressed]` rule
 *    beside one toggle is how Filter, Pin and the dashboard toggle drifted apart.
 *
 * Text choices are not icon toggles and stay out of both scans: `pill-bar-btn`,
 * `question-option`, settings choice rows and `action-btn` do not wear `icon-btn`.
 */
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

/** The sheets that paint the tones. Inside them a rule must still name a
 *  `data-tone`, or be the bar's crossfade glyph that rides the pressed state. */
const TONE_SHEETS = ['styles/global/host-components.css', 'styles/panels/shell.css'];

/** Classes an icon toggle wears. A pressed state on any other class is judged
 *  by `PRESSED_CHOICES` instead. */
const TOGGLE_CLASSES = ['icon-btn', 'pin-btn'];

/** Text choices and the call toggle's phases, which paint their own pressed
 *  state. They are not icon toggles, or refine the `live` tone. */
const PRESSED_CHOICES = ['question-option', 'pill-bar-btn', 'segmented-btn', '[data-role="call-toggle"]'];

/** Names of retired on-state classes, banned anywhere in a selector. */
const RETIRED_ON_CLASSES = /\.(?:filter-active|view-selector-active)(?![\w-])/;

/** `[aria-pressed="false"]` is the off-state and is not pressed. */
const PRESSED = /\[aria-pressed(?:=["']?true["']?)?\]/;

/** An `.active` class, which only counts on an icon toggle's own classes. */
const ACTIVE = /\.active(?![\w-])/;

function relativeToSrc(file: string): string {
  return relative(SRC, file);
}

/** `<button>` elements that wear `icon-btn` and carry `aria-pressed`. */
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

/** Selector members that paint an icon toggle's on-state outside the tone rules. */
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
  });

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
