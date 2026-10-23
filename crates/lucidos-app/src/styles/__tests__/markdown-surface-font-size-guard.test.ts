/**
 * `.markdown-content` sets no font-size, deliberately: it is the SHARED rule the
 * engine serves to app iframes as well as the host, so it carries the structural
 * markdown rules (headings, lists, tables, code) and leaves the size to whatever
 * surface wears it. Every host surface therefore names its own size, and the two
 * that do not are bare `<div class="markdown-content">`s that inherit one from a
 * sized ancestor.
 *
 * The trap is a surface that adds a wrapper class of its own and then styles
 * everything on it EXCEPT the size. Nothing complains: the block renders, and it
 * renders at the ROOT font size, which is the ui-scale percentage rather than a
 * step on the type scale. So it comes out larger than every neighbour, and
 * larger at 125% than it was at 100%, which reads as "this one view ignores the
 * type scale". That is what shipped in Settings > System > What's New: the
 * release notes were set bigger than the version heading they sat under.
 *
 * A source scan rather than a rendered test, for the same reason the sibling
 * geometry guards are: the defect is a MISSING declaration, and jsdom resolves
 * no cascade to catch it.
 */
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

/** Every file under `dir` whose name ends in `ext`, recursively. */
function filesUnder(dir: string, ext: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...filesUnder(path, ext));
    else if (entry.name.endsWith(ext)) out.push(path);
  }
  return out;
}

/** Every stylesheet the host bundle loads, concatenated. Rules are only ever
 *  asked "does one of you set this property", so one buffer is enough. */
const allCss: string = filesUnder(stylesRoot, '.css')
  .map((f: string) => readFileSync(f, 'utf-8'))
  .join('\n');

/** One `class="…"` / ``class={`…`}`` literal that names `markdown-content`. */
interface MarkdownSurface {
  file: string;
  /** The class list as written, minus any `${…}` interpolation. */
  classes: string[];
}

/** Both literal forms a class attribute takes in this codebase. A `${…}` hole is
 *  dropped rather than parsed: a surface whose sizing class is computed is not
 *  something a source scan can follow, and none exists today. */
const CLASS_ATTR = /class=(?:"([^"]*)"|\{`([^`]*)`\})/g;

function markdownSurfaces(): MarkdownSurface[] {
  const out: MarkdownSurface[] = [];
  // A `.test.tsx` fixture is markup a test builds, not a surface the app
  // renders, so its class names answer to no stylesheet. Same exclusion, for the
  // same reason, as components/shared/__tests__/list-row-prose-guard.test.ts.
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

/**
 * Whether any rule that styles this class sets a font-size.
 *
 * Answers per class rather than per surface, and memoized, because
 * `rulesTargeting` parses the whole corpus and the scan asks about the same few
 * companion classes over and over.
 *
 * It accepts a size declared in ONE context (`.pane .companion { font-size }`),
 * which the scan cannot tell apart from an unconditional one. That is the right
 * way round for a guard against a surface with NO size anywhere: a class sized
 * only where it renders is correct, and a scan strict enough to reject it would
 * have to resolve the DOM.
 */
const sizedByClass = new Map<string, boolean>();
function isSized(className: string): boolean {
  let sized = sizedByClass.get(className);
  if (sized === undefined) {
    sized = rulesTargeting(allCss, className).some(rule => rule.props.has('font-size'));
    sizedByClass.set(className, sized);
  }
  return sized;
}

describe('a markdown surface names its own font size', () => {
  it('finds the surfaces to check', () => {
    // A rename that breaks the scan must fail here rather than pass vacuously.
    expect(markdownSurfaces().length).toBeGreaterThan(5);
  });

  it('leaves the size off the shared rule, which app iframes get too', () => {
    // The bare rule only. A context-scoped override
    // (`.file-preview-content .markdown-content`) is the sanctioned way to size
    // one surface and is not what this asserts about.
    const shared = readFileSync(resolve(stylesRoot, 'global/shared-components.css'), 'utf-8');
    const bare = rulesTargeting(shared, 'markdown-content')
      .filter(rule => rule.selector === '.markdown-content');
    expect(bare.length, 'the shared markdown rule is gone').toBeGreaterThan(0);
    expect(bare.some(rule => rule.props.has('font-size'))).toBe(false);
  });

  it('sizes every surface that wraps markdown in a class of its own', () => {
    const unsized = markdownSurfaces()
      // A bare `markdown-content` has no wrapper to size and inherits from its
      // container, which no source scan can resolve. Exempt on purpose.
      .filter(s => s.classes.length > 1)
      .filter(s => !s.classes.some(c => c !== 'markdown-content' && isSized(c)))
      .map(s => `${s.file}: ${s.classes.join(' ')}`);
    expect(unsized, 'these render at the root font size, off the type scale').toEqual([]);
  });

  it('sizes the release notes, the surface this guard was written for', () => {
    expect(isSized('whats-new-notes')).toBe(true);
  });
});
