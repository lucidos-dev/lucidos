import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const STYLES = resolve(here, '..');
const SRC = resolve(STYLES, '..');
const SHELL = 'anchored-popover.css';

function imports(file: string): string[] {
  const css = readFileSync(join(STYLES, file), 'utf8');
  return [...css.matchAll(/@import\s+'\.\/([^']+)'/g)].map((m) => m[1]);
}

function sheets(dir = STYLES, prefix = ''): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e: { name: string; isDirectory(): boolean }) =>
    e.isDirectory()
      ? e.name === '__tests__' ? [] : sheets(join(dir, e.name), `${prefix}${e.name}/`)
      : e.name.endsWith('.css') ? [`${prefix}${e.name}`] : []);
}

describe('anchored popover shell cascade', () => {
  it('loads before the explainer, in global.css', () => {
    const order = imports('global.css');
    const shellAt = order.indexOf(`global/${SHELL}`);
    expect(shellAt, `global.css must import global/${SHELL}`).toBeGreaterThanOrEqual(0);
    expect(order.indexOf('global/host-components.css')).toBeGreaterThan(shellAt);
  });

  it('loads before the chat surfaces, since main.tsx imports global.css first', () => {
    const main = readFileSync(join(SRC, 'main.tsx'), 'utf8');
    expect(main.indexOf("'./styles/global.css'")).toBeGreaterThanOrEqual(0);
    expect(main.indexOf("'./styles/global.css'")).toBeLessThan(main.indexOf("'./styles/chat.css'"));
    for (const f of ['chat/waiting-indicator.css', 'chat/todo-list.css']) {
      expect(imports('chat.css'), `chat.css must import ${f}`).toContain(f);
    }
  });

  it('declares the shell rules in exactly one file', () => {
    const declaring = sheets().filter((f) =>
      /^\.anchored-popover[\w-]*[\s,{]/m.test(readFileSync(join(STYLES, f), 'utf8')));
    expect(declaring).toEqual([`global/${SHELL}`]);
  });
});
