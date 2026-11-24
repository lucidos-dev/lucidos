import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';


const here = dirname(fileURLToPath(import.meta.url));
const src: string = readFileSync(resolve(here, '../main.tsx'), 'utf8');
const gate: string = readFileSync(
  resolve(here, '../components/picker/PairingGate.tsx'),
  'utf8',
);

const codeLines: string[] = src
  .split('\n')
  .map((l: string) => l.replace(/\/\/.*$/, ''))
  .filter((l: string) => l.trim() !== '');

function loaderBody(target: string): string | null {
  const at = src.indexOf(`import('${target}')`);
  if (at === -1) return null;
  const end = src.indexOf('\n);', at);
  return end === -1 ? src.slice(at) : src.slice(at, end);
}
const pickerLoaderBody = () => loaderBody('./components/picker/WorkspacePicker');
const shellLoaderBody = () => loaderBody('./App');

function startingSplashBody(): string {
  const at = src.indexOf('function stayOnStartingSplash(');
  expect(at, 'main.tsx should declare stayOnStartingSplash').toBeGreaterThan(-1);
  const open = src.indexOf('{', at);
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === '{') depth++;
    else if (src[i] === '}' && --depth === 0) return src.slice(open + 1, i);
  }
  throw new Error('unbalanced braces in stayOnStartingSplash');
}

describe('boot ownership handover', () => {
  it('keeps the picker code-split (the premise of the rest of this suite)', () => {
    expect(pickerLoaderBody(), 'the picker should load via lazyComponent(() => import(...))').not.toBeNull();
    expect(src).toMatch(/const WorkspacePicker = lazyComponent\(/);
  });

  it('never hands over at the top level', () => {
    const topLevel = codeLines.filter(
      (l: string) => /^\S/.test(l) && /(handOverBootOwnership|__lucidosBootLoaded\?\.)\(\)/.test(l),
    );
    expect(topLevel, 'each path hands over once its own root is in memory').toEqual([]);
  });

  it('keeps the shell code-split, and hands over once its chunk resolves', () => {
    expect(src).toMatch(/const App = lazyComponent\(/);
    expect(src, 'a static import would put the shell back in the entry chunk').not.toMatch(
      /^import \{ App \} from '\.\/App';$/m,
    );
    expect(
      /handOverBootOwnership\(\);/.test(shellLoaderBody() ?? ''),
      'the app path must hand over once the shell chunk resolves',
    ).toBe(true);
  });

  it('hands over on the pre-gateway desktop splash, whose root is the entry itself', () => {
    expect(startingSplashBody()).toMatch(/handOverBootOwnership\(\);/);
  });

  it('hands over from inside the picker lazy loader', () => {
    expect(
      /handOverBootOwnership\(\);/.test(pickerLoaderBody() ?? ''),
      'the picker path must hand over once its own chunk resolves',
    ).toBe(true);
  });
});

describe('the pairing screen owns its own boot', () => {
  it('hands over once, and only for the unpaired state', () => {
    const calls = gate.match(/handOverBootOwnership\(\)/g) ?? [];
    expect(calls, 'exactly one handover site in the gate').toHaveLength(1);
    expect(
      /if \(state === 'unpaired'\) handOverBootOwnership\(\);/.test(gate),
      "gate the handover on 'unpaired': the other states still render the lazy picker",
    ).toBe(true);
  });

  it('dismisses the boot splash, which otherwise covers the form', () => {
    expect(
      /dismissBootSplash\(\)/.test(gate),
      'nothing else dismisses the splash on this path',
    ).toBe(true);
  });

  it('leaves no screen it can paint under the cover', () => {
    const uses = gate.match(/useUncoverOnPaint\(/g) ?? [];
    expect(uses, 'one per screen the gate can paint, plus the definition').toHaveLength(5);
    expect(
      /dismissBootSplash\(\)/.test(gate.slice(gate.indexOf('function useUncoverOnPaint'))),
      'the hook is the only thing that lifts the cover',
    ).toBe(true);
    const raw = gate.match(/dismissBootSplash\(\)/g) ?? [];
    expect(raw, 'no screen may call it around the hook').toHaveLength(1);
  });

  it('uncovers on paint, not on mount, wherever a branch can draw nothing', () => {
    expect(gate).toMatch(/useUncoverOnPaint\(auto === 'running' && showBusy\);/);
    expect(gate).toMatch(/useUncoverOnPaint\(autoPair === 'done' \|\| showAutoPair\);/);
  });
});
