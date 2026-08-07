/**
 * Shared reader for the CSS source scans. Several suites pin a geometry
 * contract by asserting on the declarations a rule actually carries (the
 * below-header anchor, the per-pane toast columns, the transcript fades), and
 * each had grown its own byte-identical copy of these two functions.
 *
 * postcss is the right tool when a scan needs EVERY rule in a sheet, or the
 * at-rules a rule is nested inside (`hooks/useThreadScrollIndicator.test.ts`
 * parses that way, and `engine-served-css-parses.test.ts` uses it as the parse
 * gate for the engine-served sheet). These two are for the far commoner case:
 * one rule you can name, and a handful of its declarations.
 */
import { expect } from 'vitest';

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
