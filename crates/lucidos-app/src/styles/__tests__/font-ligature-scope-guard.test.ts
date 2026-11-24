import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../../..');

function read(relPath: string): string {
  return readFileSync(resolve(REPO_ROOT, relPath), 'utf8');
}

const CONSUMERS = [
  'crates/lucidos-app/src/styles/global/base.css',
  'crates/lucidos-agents-api/src/api/sdk_iframe.css',
] as const;

describe('a `font` shorthand does not silently re-enable ligatures', () => {
  const CSS_ROOTS = [
    'crates/lucidos-app/src/styles',
    'crates/lucidos-app/src/components',
    'crates/lucidos-agents-api/src/api',
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
