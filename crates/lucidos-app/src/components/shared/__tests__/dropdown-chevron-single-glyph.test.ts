// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, readFileSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';

function tsxFiles(dir: string): string[] {
  return (readdirSync(dir) as string[]).flatMap(name => {
    const path = `${dir}/${name}`;
    if (statSync(path).isDirectory()) return tsxFiles(path);
    return path.endsWith('.tsx') && !path.includes('__tests__') ? [path] : [];
  });
}

it('draws no up-pointing chevron in any component', () => {
  const src: string = fileURLToPath(new URL('../../..', import.meta.url)).replace(/\/$/, '');
  const files = tsxFiles(src);
  expect(files.some(path => path.endsWith('/components/shared/Dropdown.tsx'))).toBe(true);
  expect(files.filter(path => readFileSync(path, 'utf8').includes('▴'))).toEqual([]);
});
