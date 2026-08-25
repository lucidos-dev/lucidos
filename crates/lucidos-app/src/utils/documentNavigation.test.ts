import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { isPlainClick, replaceDocument, replaceOnPlainClick } from './documentNavigation';

// The node test env has no window.location (test-setup.ts aliases window to
// globalThis), so it is installed as a global, matching openExternalUrl.test.ts.
let fakeLocation: { replace: ReturnType<typeof vi.fn> };

beforeEach(() => {
  fakeLocation = { replace: vi.fn() };
  vi.stubGlobal('location', fakeLocation);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('replaceDocument', () => {
  it('replaces the current history entry instead of pushing one', () => {
    replaceDocument('/loopws/');
    expect(fakeLocation.replace).toHaveBeenCalledWith('/loopws/');
  });
});

describe('isPlainClick', () => {
  it('accepts a bare left click', () => {
    expect(isPlainClick({ button: 0 })).toBe(true);
  });

  it('accepts a click that reports no button at all', () => {
    expect(isPlainClick({})).toBe(true);
  });

  it.each(['metaKey', 'ctrlKey', 'shiftKey', 'altKey'] as const)(
    'declines a %s click, which the browser opens in a new tab or window',
    (mod) => {
      expect(isPlainClick({ button: 0, [mod]: true })).toBe(false);
    },
  );

  it('declines a middle click', () => {
    expect(isPlainClick({ button: 1 })).toBe(false);
  });

  it('declines a click somebody else already cancelled', () => {
    expect(isPlainClick({ button: 0, defaultPrevented: true })).toBe(false);
  });
});

/** A click of the shape `replaceOnPlainClick` reads, with a recording
 *  `preventDefault`. */
function clickEvent(over: Partial<MouseEvent> = {}) {
  return {
    button: 0,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    defaultPrevented: false,
    preventDefault: vi.fn(),
    ...over,
  } as unknown as MouseEvent & { preventDefault: ReturnType<typeof vi.fn> };
}

describe('replaceOnPlainClick', () => {
  it('cancels a plain click and replaces the document', () => {
    const e = clickEvent();
    replaceOnPlainClick('/~/')(e);
    expect(e.preventDefault).toHaveBeenCalled();
    expect(fakeLocation.replace).toHaveBeenCalledWith('/~/');
  });

  it('leaves a cmd-click to the browser, so it still opens a new tab', () => {
    const e = clickEvent({ metaKey: true });
    replaceOnPlainClick('/~/')(e);
    expect(e.preventDefault).not.toHaveBeenCalled();
    expect(fakeLocation.replace).not.toHaveBeenCalled();
  });

  it('runs `before` on every click, cmd-click included (it shuts the menu)', () => {
    const before = vi.fn();
    replaceOnPlainClick('/~/', before)(clickEvent());
    replaceOnPlainClick('/~/', before)(clickEvent({ metaKey: true }));
    expect(before).toHaveBeenCalledTimes(2);
  });
});

// Source-scan tripwire: no `location.href` assignment outside the one file that
// deliberately leaves the app.
//
// Assigning it PUSHES a history entry. A workspace left on the back stack is
// what turns a hole in the mobile edge guard into a silent teleport into that
// workspace. Reads are fine, and common (`new URL(location.href)`), so the scan
// matches an assignment only.

const here: string = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '..');

/** Leaves the app on purpose, through iOS's `x-safari-` scheme. */
const ALLOWED = ['utils/openExternalUrl.ts'];

const HREF_ASSIGNMENT = /location\.href\s*=(?!=)/;

/** Every non-test frontend source, comment-stripped so the scan cannot mistake
 *  prose about the ban for code. */
function frontendSources(): Array<[string, string]> {
  const walk = (dir: string): string[] =>
    readdirSync(dir, { withFileTypes: true }).flatMap((e: any) => {
      const full = resolve(dir, e.name);
      if (e.isDirectory()) return walk(full);
      if (!/\.tsx?$/.test(full) || /\.test\.tsx?$/.test(full)) return [];
      return [full];
    });
  return walk(SRC).map((p: string) => [
    relative(SRC, p).split('\\').join('/'),
    readFileSync(p, 'utf-8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, ''),
  ]);
}

describe('no location.href assignment outside the sanctioned escape hatch', () => {
  it('finds sources to scan at all', () => {
    expect(frontendSources().length).toBeGreaterThan(100);
  });

  it('the allow-listed file is the only assigner left', () => {
    const found = frontendSources()
      .filter(([, src]) => HREF_ASSIGNMENT.test(src))
      .map(([path]) => path);
    expect(found).toEqual(ALLOWED);
  });
});
