import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
// @ts-expect-error — same
import { dirname, resolve, relative } from 'node:path';

/**
 * Enforces the self-skeletonizing skeleton convention (see
 * `.claude/rules/frontend.md` § "Loadable surfaces ship a self-skeletonizing
 * skeleton"). The test infra has no jsdom, so this is a source-scan, not a render
 * test:
 *
 *  - Guard A: the retired generic-bar list skeleton component must stay gone —
 *    nothing may reintroduce a bare `ListSkeleton` symbol (only `ListSkeletonOf`).
 *  - Guard B: every component that wires a `<LoadingFade>` must pair it with a
 *    self-skeletonizing skeleton — `ListSkeletonOf`, a `SkeletonProvider`, or a
 *    `*Skeleton` component built on the shared `Skeleton.tsx` primitives — not a
 *    hand-rolled inline element.
 *
 * Test files are excluded from the scan (the precedent in `no-raw-storage.test.ts`),
 * so this file's own prose mentions don't trip the guard.
 */

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../../..'); // crates/lucidos-app/src
const COMPONENTS = resolve(SRC, 'components');

function sourceFiles(dir: string, exts: string[]): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) out.push(...sourceFiles(full, exts));
    else if (exts.some((e) => entry.name.endsWith(e)) && !/\.test\.tsx?$/.test(entry.name)) out.push(full);
  }
  return out;
}

// `\bListSkeleton\b` does NOT match `ListSkeletonOf` (the trailing "Of" blocks the
// word boundary) nor `showListSkeleton` (no boundary before the captured word).
const RETIRED_GENERIC = /\bListSkeleton\b/;

// A LoadingFade's skeleton must reference one of these self-skeletonizing forms.
const APPROVED_SKELETON = /ListSkeletonOf|SkeletonProvider|\b\w*Skeleton\b|folderTreeSkeletonRow/;

describe('skeleton convention guard', () => {
  it('does not reintroduce the retired generic-bar list skeleton (only ListSkeletonOf)', () => {
    const offenders = sourceFiles(SRC, ['.ts', '.tsx'])
      .filter((f) => RETIRED_GENERIC.test(readFileSync(f, 'utf8')))
      .map((f) => relative(SRC, f));
    expect(offenders, 'use ListSkeletonOf (self-skeletonizing), not the retired generic list skeleton').toEqual([]);
  });

  it('every <LoadingFade> is paired with a self-skeletonizing skeleton', () => {
    const offenders = sourceFiles(COMPONENTS, ['.tsx'])
      .filter((f) => {
        const src = readFileSync(f, 'utf8');
        return src.includes('<LoadingFade') && !APPROVED_SKELETON.test(src);
      })
      .map((f) => relative(SRC, f));
    expect(
      offenders,
      'a Loadable surface using <LoadingFade> must pass a self-skeletonizing skeleton (ListSkeletonOf / a *Skeleton built on Skeleton.tsx primitives)',
    ).toEqual([]);
  });
});
