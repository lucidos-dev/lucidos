
import { expect } from 'vitest';

export function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\])\/\/.*$/gm, '$1');
}

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
