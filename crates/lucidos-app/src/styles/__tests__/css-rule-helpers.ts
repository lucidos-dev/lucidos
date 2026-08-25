/**
 * Shared reader for the CSS source scans. Several suites pin a geometry
 * contract by asserting on the declarations a rule actually carries (the
 * below-header anchor, the per-pane toast columns, the transcript fades), and
 * each had grown its own byte-identical copy of the two string functions.
 *
 * Two readers here, and picking the wrong one is the trap this header exists
 * for. `styleSheetPaths` feeds either one when the scan is over every sheet
 * rather than a named file.
 *
 * `block` + `decl` resolve the FIRST TEXTUAL match, which is right for the
 * common case: one rule you can name, and a handful of its declarations. It is
 * wrong the moment a sheet can override that rule elsewhere, because a first
 * match reads neither an `@media` copy below it nor a compound selector that
 * beats it on specificity, and both look like a passing scan.
 *
 * `cssRules` + `rulesTargeting` parse with postcss instead, and are the tool
 * when the assertion is about a rule NOT being overridden anywhere, or about
 * the at-rules a rule is nested inside. Nesting depth, comments and strings
 * come out correct for free rather than by counting braces.
 * (`engine-served-css-parses.test.ts` uses postcss directly, as a parse gate
 * rather than a reader.)
 */
import { expect } from 'vitest';
import postcss, { type AtRule, type Container, type Declaration, type Document } from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { resolve } from 'node:path';

/** Every stylesheet under `root`, recursively, as absolute paths. It skips
 *  `__tests__` so a fixture never reads as shipping CSS. Shared because a scan
 *  asking "does any sheet do X" has to see all of them, and three suites had
 *  grown the same walk. */
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

/** Body of the first rule/at-rule block whose header matches `needle`, starting
 *  the search at `from`. Brace-matched, so a nested block (a `:root` inside a
 *  `@media`) is returned whole instead of being cut at the first `}`. */
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

/** A declaration's value, or null when the block doesn't set that property. */
export function decl(body: string, prop: string): string | null {
  const m = body.match(new RegExp(`(?:^|[;{]|\\*/)\\s*${prop}:\\s*([^;]+);`));
  return m ? m[1].replace(/\s+/g, ' ').trim() : null;
}

/** One style rule, with the at-rule preludes it is nested inside. */
export interface CssRule {
  /** The rule's own selector list, whitespace-collapsed. */
  selector: string;
  /** `@media …` ancestors, outermost first, space-joined; empty at top level. */
  atRules: string;
  /** Declarations in source order as `prop: value`, joined by `; `. */
  body: string;
  /** The same declarations keyed by property, last one within the rule winning. */
  props: Map<string, string>;
}

/** Every style rule in the sheet, in source order. */
export function cssRules(css: string): CssRule[] {
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

/**
 * A rule's selector list, one member per entry, trimmed.
 *
 * Use this rather than `selector.split(',')` for the same reason
 * `rulesTargeting` does below: a plain split cannot tell a selector-list comma
 * from one inside `:is(.a, .b)`, so it hands back two halves of one member and
 * a scan asking "does this rule name `.x`" answers against a selector nobody
 * wrote. Exported because asserting on the MEMBERS of a grouped rule is its own
 * common shape, separate from asking which rules target a class.
 */
export function selectorList(selector: string): string[] {
  return postcss.list.comma(selector).map(s => s.trim());
}

/**
 * Top-level combinators replaced by spaces, so the subject split below can be
 * a plain space split. Depth-aware, so `:is(.a > .b)` keeps its inner
 * combinator and `[class~="x"]` keeps its `~`.
 */
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

/**
 * `:has(…)` arguments removed, parens balanced by depth so a nested one goes
 * with its owner.
 *
 * Every other functional pseudo-class matches its argument against the SUBJECT
 * (`:is(.a, .b)`, `:where(…)`, `:not(…)` all still describe the element the
 * rule styles), so their contents must stay. `:has()` is the one that matches
 * against something ELSE: `.turn:has(.header)` styles the turn precisely
 * BECAUSE it contains a header, and styles the header not at all. Left in, a
 * class named there reads as the subject, and a scan asking "what styles
 * `.header`" answers with a rule that never touches it. That is not
 * hypothetical: the transcript's tail room was reserved on
 * `.chat-exchange:last-child:has(.response-header)`, and its `min-height` was
 * read as the response header's own. (That rule is gone, the room with it, but
 * the trap it sprang is a property of `:has()` and stays.)
 */
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

/**
 * Every rule that styles the ELEMENT carrying `className`, in source order.
 *
 * The subject of a selector is its last compound, so this keeps
 * `.a .target`, `.target.state` and a bare `.target` (all of which style the
 * element) and drops `.target .child` (which styles a descendant),
 * `.target::-webkit-scrollbar` (which styles a pseudo-element, not the box)
 * and `.other:has(.target)` (which styles whatever CONTAINS the target; see
 * `stripHas`).
 * That distinction is the whole point: a scan asserting "nothing re-enables X"
 * has to see the compound and descendant-combinator forms that outrank the
 * bare rule, and must not trip over rules aimed at children.
 *
 * Split with postcss's own list tokenizers rather than `String.split`, which
 * cannot tell a selector-list comma from one inside `:is(.a, .b)`.
 */
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
