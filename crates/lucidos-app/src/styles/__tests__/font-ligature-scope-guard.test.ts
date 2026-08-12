/**
 * Fira Code's programming ligatures must stay scoped to CODE, never reach prose.
 *
 * Fira Code's `calt` deliberately re-spaces dot runs (`..` and `...` are
 * separate contextual rules so the font can tighten range operators), so a typed
 * `...` comes out kerned tight enough to read as two dots in the composer and in
 * the transcript. That is tonsky/FiraCode#1561, open upstream, with no way to
 * disable one sequence, so the only remedy is turning the feature off for
 * natural-language text and back on for code.
 *
 * The mechanism is a two-part contract, and BOTH halves have to hold:
 *
 *  1. Something publishes two custom properties, `--font-features-text`
 *     (explicitly OFF for Fira Code) and `--font-features-code` (explicitly ON).
 *     A custom property inherits as a *value* and renders nothing until a rule
 *     applies it, so publishing is inert and CSS alone decides scope.
 *  2. Exactly two stylesheets consume them: the text value on
 *     `html, input, textarea, select, button`, the code value on the code
 *     elements.
 *
 * **This file now guards only half (2).** Half (1) used to be four hand-copied
 * set-points, which is what most of this guard was for; they are one function
 * in `@lucidos/appearance` now, and `appearance.test.ts` owns its rules
 * (including that the OFF value is the explicit zeros, never `normal`).
 * `appearanceBoot.test.ts` drives what actually reaches `<html>`.
 *
 * What is left is the CSS side, which no amount of deduplication reaches,
 * and its two failure modes are the subtle ones:
 *
 *  a. Leaving form controls out of the text rule. A `<textarea>` does not inherit
 *     `font-feature-settings`, because the UA stylesheet's `font` shorthand
 *     resets it. `html` alone fixes prose and leaves the composer, which is the
 *     surface the bug was actually reported against, untouched.
 *  b. A consumer applying the CODE value at `:root`/`html`/`body`/`*`, which
 *     re-inherits the ligatures to everything through the stylesheet instead.
 *
 * Plus the `font:` shorthand sweep below, which is the same trap a third way:
 * the shorthand silently resets the property to `normal`, re-enabling on 15
 * sites what their own authors were trying to inherit.
 *
 * All of these look RIGHT in `getComputedStyle(el).fontFeatureSettings`. They
 * were settled by pixel comparison in headless Chromium against the real
 * webfont. Do not "verify" a change here by reading the computed value.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
/** Repo root, from `crates/lucidos-app/src/styles/__tests__/`. */
const REPO_ROOT = resolve(here, '../../../../..');

function read(relPath: string): string {
  return readFileSync(resolve(REPO_ROOT, relPath), 'utf8');
}

/** Every place allowed to turn the published value into rendering. */
const CONSUMERS = [
  'crates/lucidos-app/src/styles/global/base.css',
  'crates/lucidos-engine/src/api/sdk_iframe.css',
] as const;

describe('a `font` shorthand does not silently re-enable ligatures', () => {
  // `font` is a shorthand, and per CSS Fonts it resets `font-feature-settings`
  // to its INITIAL value (`normal`) even though the shorthand cannot set it.
  // `normal` means "the font's defaults", and liga/calt are default-ON, so on a
  // Fira Code user every `font: inherit` reset re-enables the ligatures its own
  // author was trying to inherit. That is not theoretical: 15 sites did it,
  // including `.inline-step .step-main`, where tool descriptions ending in `...`
  // render throughout the transcript.
  //
  // The fix at each site is `font-feature-settings: inherit`, which is what the
  // author meant by `font: inherit` anyway. This guard keeps the next one honest.
  const CSS_ROOTS = [
    'crates/lucidos-app/src/styles',
    'crates/lucidos-engine/src/api',
  ] as const;

  function cssFiles(dir: string): string[] {
    const abs = resolve(REPO_ROOT, dir);
    const out: string[] = [];
    for (const entry of readdirSync(abs, { withFileTypes: true })) {
      const rel = `${dir}/${entry.name}`;
      if (entry.isDirectory()) out.push(...cssFiles(rel));
      else if (entry.name.endsWith('.css')) out.push(rel);
    }
    return out;
  }

  it('every `font:` shorthand restores font-feature-settings', () => {
    const offenders: string[] = [];
    for (const root of CSS_ROOTS) {
      for (const relPath of cssFiles(root)) {
        const lines = read(relPath).split('\n');
        lines.forEach((line, i) => {
          if (!/^\s*font:\s/.test(line)) return;
          // The declaration must be followed, within the same rule block, by an
          // explicit font-feature-settings. Look at the next few lines rather
          // than only the next one, so a comment between them is fine.
          const following = lines.slice(i + 1, i + 6).join('\n');
          const upToBlockEnd = following.split('}')[0];
          if (!upToBlockEnd.includes('font-feature-settings')) {
            offenders.push(`${relPath}:${i + 1}  ${line.trim()}`);
          }
        });
      }
    }
    expect(
      offenders,
      'A `font:` shorthand resets font-feature-settings to `normal`, which is '
      + 'NOT "off" (liga and calt are default-ON), so these sites give a Fira '
      + 'Code user ligatures back on prose. Add `font-feature-settings: inherit;` '
      + 'right after each:\n' + offenders.join('\n'),
    ).toEqual([]);
  });
});

describe('Fira Code ligatures stay scoped to code surfaces', () => {
  for (const relPath of CONSUMERS) {
    it(`consumes the custom property on code surfaces: ${relPath}`, () => {
      const src = read(relPath);

      expect(
        src.includes('var(--font-features-code'),
        `${relPath} is where the ligatures are actually applied. Without it the published `
        + 'value renders nothing and Fira Code loses its ligatures in code blocks '
        + 'and diffs too, which is not the fix.',
      ).toBe(true);

      // Failure (b): a <textarea> does not inherit font-feature-settings, because
      // the UA stylesheet's `font` shorthand resets it. The text rule must name
      // the form controls, or the composer keeps its ligatures while prose loses
      // them, which is precisely the half-fix that shipped.
      const textRule = /(^|[},/\s])((html|input|textarea|select|button)\s*,\s*)*(html|input|textarea|select|button)\s*\{[^}]*var\(--font-features-text/;
      const m = src.match(textRule);
      expect(
        m !== null,
        `${relPath} has no rule applying var(--font-features-text) to html and the form `
        + 'controls. Without it nothing turns the ligatures off.',
      ).toBe(true);
      for (const sel of ['html', 'textarea', 'input']) {
        expect(
          (m?.[0] ?? '').includes(sel),
          `${relPath}'s text rule does not cover \`${sel}\`. A <textarea> does not inherit `
          + "font-feature-settings (the UA stylesheet's `font` shorthand resets it), so "
          + 'the composer keeps its ligatures unless the rule names it.',
        ).toBe(true);
      }

      // The rule is only correct if it is scoped. A consumer that applied it at
      // `:root`, `html`, `body` or `*` would re-inherit the feature to
      // everything and reintroduce the original bug through the stylesheet
      // instead of the inline style. `*` is in the list because it is the one
      // selector that globalizes without naming a global element, so it is the
      // easiest way to reintroduce this by accident.
      const globalScope = /(^|[},/\s])(:root|html|body|\*)\s*(,[^{]*)?\{[^}]*var\(--font-features-code/;
      expect(
        globalScope.test(src),
        `${relPath} applies --font-features-code at :root/html/body/* scope. `
        + 'font-feature-settings inherits, so that puts the ligatures back on every '
        + 'character in the app. Keep the rule on code, pre, kbd, samp and friends.',
      ).toBe(false);
    });
  }
});
