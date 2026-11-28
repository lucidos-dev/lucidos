// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { join, resolve } from 'node:path';

function markdownUnder(dir: string): string[] {
  return (readdirSync(dir) as string[]).sort().flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return markdownUnder(path);
    return name.endsWith('.md') ? [path] : [];
  });
}

export function docFamilyText(repo: string, rel: string): string {
  const doc = resolve(repo, rel);
  const folder = doc.replace(/\.md$/, '');
  const parts = folder !== doc && existsSync(folder) ? markdownUnder(folder) : [];
  return [doc, ...parts].map((path) => readFileSync(path, 'utf8') as string).join('\n');
}
