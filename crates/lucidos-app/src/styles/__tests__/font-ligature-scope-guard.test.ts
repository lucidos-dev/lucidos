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
 *  1. Four set-points publish two custom properties, `--font-features-text`
 *     (explicitly OFF for Fira Code) and `--font-features-code` (explicitly ON).
 *     A custom property inherits as a *value* and renders nothing until a rule
 *     applies it, so publishing is inert and CSS alone decides scope.
 *  2. Exactly two stylesheets consume them: the text value on
 *     `html, input, textarea, select, button`, the code value on the code
 *     elements.
 *
 * FOUR WAYS TO BREAK IT, and this guard exists for all four. The first two are
 * not hypothetical: they shipped together on 2026-08-07 as a change that read
 * correctly, passed its tests, and did nothing at all.
 *
 *  a. Spelling the OFF value `normal`. `liga` and `calt` are default-ON features
 *     in CSS, so `normal` means "the font's defaults" and renders BYTE-IDENTICALLY
 *     to `"liga" 1, "calt" 1`. Merely removing the enabling declaration is a
 *     no-op. Only the explicit zeros disable them.
 *  b. Leaving form controls out of the text rule. A `<textarea>` does not inherit
 *     `font-feature-settings`, because the UA stylesheet's `font` shorthand
 *     resets it. `html` alone fixes prose and leaves the composer, which is the
 *     surface the bug was actually reported against, untouched.
 *  c. A set-point regressing to bare `font-feature-settings` on the document
 *     element, which puts scope back in JS where the stylesheets cannot see it.
 *  d. A consumer applying the CODE value at `:root`/`html`/`body`/`*`, which
 *     re-inherits the ligatures to everything through the stylesheet instead.
 *
 * Note what (a) and (b) have in common: `getComputedStyle(el).fontFeatureSettings`
 * looks right in both. They were settled by pixel comparison in headless
 * Chromium against the real webfont. Do not "verify" a change here by reading
 * the computed value.
 *
 * The set-points cannot share code: two are inline FOUC scripts (one in
 * `index.html`, one an `include_str!`d Rust string literal served as
 * `sdk-prefs.js`) that run before any bundle loads, one is the host store, one
 * is the SDK an app iframe imports. Duplication is forced, so the guard is the
 * thing keeping them in lockstep.
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

/** Every place that writes the ligature value onto the document element. */
const SET_POINTS = [
  'crates/lucidos-app/src/store/actions/preferences.ts',
  'crates/lucidos-app/index.html',
  'crates/lucidos-engine/src/api/sdk_prefs.rs',
  'packages/lucidos-sdk/src/ui.ts',
] as const;

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
  for (const relPath of SET_POINTS) {
    it(`publishes the custom property, not the inherited one: ${relPath}`, () => {
      const src = read(relPath);

      for (const prop of ['--font-features-text', '--font-features-code']) {
        expect(
          src.includes(prop),
          `${relPath} is one of the ${SET_POINTS.length} set-points that publish the ligature `
          + `values; it must write ${prop} so the stylesheets decide where the `
          + 'feature applies.',
        ).toBe(true);
      }

      // Failure (a): the OFF value must be the explicit zeros. `normal` is "the
      // font's defaults", and liga/calt are default-ON, so an off value spelled
      // `normal` renders byte-identically to `1` and the whole change is inert.
      // (`normal` is still correct for the non-Fira FALLBACK, which does want
      // the defaults; this asserts the Fira value specifically.)
      expect(
        /["']liga["']\s+0\s*,\s*["']calt["']\s+0/.test(src),
        `${relPath} does not publish an explicit "liga" 0, "calt" 0 as the text value. `
        + '`normal` does NOT disable ligatures: liga and calt are default-ON in CSS, '
        + 'so `normal` renders identically to `1` and a typed "..." still collapses.',
      ).toBe(true);

      // Both spellings of the same mistake. `setProperty('font-feature-settings',
      // …)` is how all four set-points write today, quoted '...' in the TS/JS
      // ones and "..." inside the Rust string literal; matching the property
      // name plus its closing quote keeps the check off prose and comments that
      // merely mention it. `style.fontFeatureSettings = …` is the IDL spelling,
      // equally available and equally global, so a guard that only knew the
      // first would wave the regression through in a different disguise.
      const bareProperty = /setProperty\(\s*['"]font-feature-settings['"]|\.fontFeatureSettings\s*=/;
      expect(
        bareProperty.test(src),
        `${relPath} sets font-feature-settings directly on the document element. That `
        + "property is inherited, so it ligatures prose too, and Fira Code's calt "
        + 'collapses a typed "..." into what reads as two dots. Publish '
        + '--font-features-code instead and let the code-surface rule consume it.',
      ).toBe(false);
    });
  }

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
