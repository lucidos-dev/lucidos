/** Helpers for the guards that read startup code rather than run it. */

import { expect } from 'vitest';

/** Strip `//` and block comments, so a surviving comment can never stand in for
 *  a deleted call.
 *
 *  A `//` after a backslash stays. It closes a regex literal such as
 *  `/^https?:\/\//`, and reading it as a comment would drop the rest of the
 *  line. */
export function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\])\/\/.*$/gm, '$1');
}

/** The body of a `function <name>(…)` declaration, by brace matching from its
 *  opening `{`. Comments are stripped first, so a brace inside one cannot skew
 *  the match. */
export function handlerBody(src: string, declaration: string): string {
  const stripped = stripComments(src);
  const start = stripped.indexOf(declaration);
  expect(start, `the source must declare \`${declaration}\``).toBeGreaterThan(-1);
  const open = stripped.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < stripped.length; i++) {
    if (stripped[i] === '{') depth++;
    else if (stripped[i] === '}' && --depth === 0) return stripped.slice(open + 1, i);
  }
  throw new Error(`unbalanced braces in \`${declaration}\`, so the guard cannot bound the handler`);
}
