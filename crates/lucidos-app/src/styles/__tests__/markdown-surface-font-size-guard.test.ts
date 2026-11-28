import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, join, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const srcRoot: string = resolve(here, '../..');
const stylesRoot: string = resolve(here, '..');

function filesUnder(dir: string, ext: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...filesUnder(path, ext));
    else if (entry.name.endsWith(ext)) out.push(path);
  }
  return out;
}

const allCss: string = filesUnder(stylesRoot, '.css')
  .map((f: string) => readFileSync(f, 'utf-8'))
  .join('\n');

interface MarkdownSurface {
  file: string;
  classes: string[];
}

const CLASS_ATTR = /class=(?:"([^"]*)"|\{`([^`]*)`\})/g;

function markdownSurfaces(): MarkdownSurface[] {
  const out: MarkdownSurface[] = [];
  for (const file of filesUnder(srcRoot, '.tsx').filter((f: string) => !f.endsWith('.test.tsx'))) {
    const source: string = readFileSync(file, 'utf-8');
    for (const m of source.matchAll(CLASS_ATTR)) {
      const raw = (m[1] ?? m[2]).replace(/\$\{[^}]*\}/g, ' ');
      const classes = raw.split(/\s+/).filter(Boolean);
      if (classes.includes('markdown-content')) {
        out.push({ file: relative(srcRoot, file), classes });
      }
    }
  }
  return out;
}

const sizedByClass = new Map<string, boolean>();
function isSized(className: string): boolean {
  let sized = sizedByClass.get(className);
  if (sized === undefined) {
    sized = rulesTargeting(allCss, className).some(rule => rule.props.has('font-size'));
    sizedByClass.set(className, sized);
  }
  return sized;
}

const NOT_A_WRAPPER = new Set(['markdown-content', 'authored-html']);

describe('a markdown surface names its own font size', () => {
  it('finds the surfaces to check', () => {
    expect(markdownSurfaces().length).toBeGreaterThan(5);
  });

  it('leaves the size off the shared rule, which app iframes get too', () => {
    const shared = readFileSync(resolve(stylesRoot, 'global/shared-components.css'), 'utf-8');
    const bare = rulesTargeting(shared, 'markdown-content')
      .filter(rule => rule.selector === '.markdown-content');
    expect(bare.length, 'the shared markdown rule is gone').toBeGreaterThan(0);
    expect(bare.some(rule => rule.props.has('font-size'))).toBe(false);
  });

  it('sizes every surface that wraps markdown in a class of its own', () => {
    const unsized = markdownSurfaces()
      .map(s => ({ ...s, wrappers: s.classes.filter(c => !NOT_A_WRAPPER.has(c)) }))
      .filter(s => s.wrappers.length > 0)
      .filter(s => !s.wrappers.some(isSized))
      .map(s => `${s.file}: ${s.classes.join(' ')}`);
    expect(unsized, 'these render at the root font size, off the type scale').toEqual([]);
  });

  it('sizes the release notes, the surface this guard was written for', () => {
    expect(isSized('whats-new-notes')).toBe(true);
  });
});
