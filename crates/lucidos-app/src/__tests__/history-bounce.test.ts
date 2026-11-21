/**
 * The installed iOS PWA steps forward off a page it reached by going back.
 *
 * Every iOS notification tap loads a new document, so the page the user was on
 * stays one entry back. The left-edge swipe is iOS's back gesture, and the edge
 * guard cannot cover every touch, so a stray swipe lands there. This inline
 * `<head>` script is what sends the user straight back to the live page. It is
 * lifted out of `index.html` and run with stub globals, as the cold-start fast
 * path suite does.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const INDEX_HTML: string = readFileSync(resolve(here, '../../index.html'), 'utf8');

function bounceSource(): string {
  const at = INDEX_HTML.indexOf('lucidosHistoryBounce = true');
  expect(at, 'no inline script steps forward off a traversed page').toBeGreaterThanOrEqual(0);
  const open = INDEX_HTML.lastIndexOf('<script>', at);
  const close = INDEX_HTML.indexOf('</script>', at);
  return INDEX_HTML.slice(open + '<script>'.length, close);
}

interface Env {
  /** `navigator.standalone`: true only in a Safari web app (iOS home screen or macOS Dock). */
  standalone?: boolean;
  /** The navigation entry's `type`, or null for no entry. */
  navType?: string | null;
  /** `navigation.canGoForward`, or undefined where the Navigation API is missing. */
  canGoForward?: boolean;
}

interface Run {
  forwards: number;
  flagged: boolean;
  /** Fire `pageshow`, as a back-forward cache restore or an ordinary show. */
  pageshow: (persisted: boolean) => void;
}

function run(env: Env): Run {
  const { standalone = true, navType = 'navigate', canGoForward } = env;
  const result = { forwards: 0 };
  const listeners: Array<(e: { persisted: boolean }) => void> = [];
  const win: Record<string, unknown> = canGoForward === undefined ? {} : { navigation: { canGoForward } };
  // The script under test is inline HTML, so only `new Function` can run it.
  // eslint-disable-next-line no-new-func
  new Function('navigator', 'performance', 'history', 'addEventListener', 'window', bounceSource())(
    { standalone },
    { getEntriesByType: () => (navType === null ? [] : [{ type: navType }]) },
    { forward: () => void result.forwards++ },
    (type: string, fn: (e: { persisted: boolean }) => void) => { if (type === 'pageshow') listeners.push(fn); },
    win,
  );
  return {
    get forwards() { return result.forwards; },
    get flagged() { return win.lucidosHistoryBounce === true; },
    pageshow: (persisted) => listeners.forEach((fn) => fn({ persisted })),
  };
}

describe('iOS PWA history bounce', () => {
  it('steps forward off a page loaded by a back swipe', () => {
    const r = run({ navType: 'back_forward' });
    expect(r.forwards).toBe(1);
    expect(r.flagged).toBe(true);
  });

  it('steps forward off a page restored from the back-forward cache', () => {
    const r = run({});
    expect(r.forwards).toBe(0);
    r.pageshow(false);
    expect(r.forwards).toBe(0);
    r.pageshow(true);
    expect(r.forwards).toBe(1);
  });

  it('leaves an ordinary load and a reload alone', () => {
    expect(run({ navType: 'navigate' }).forwards).toBe(0);
    expect(run({ navType: 'reload' }).forwards).toBe(0);
    expect(run({ navType: null }).forwards).toBe(0);
  });

  it('does nothing outside a Safari web app, where back is the user\'s', () => {
    const r = run({ standalone: false, navType: 'back_forward' });
    r.pageshow(true);
    expect(r.forwards).toBe(0);
    expect(r.flagged).toBe(false);
  });

  it('stays put when there is nowhere forward to go', () => {
    // A restored session reloads its current entry as a traversal. Flagging it
    // would stand the picker's redirect down for a hop that never happens.
    const r = run({ navType: 'back_forward', canGoForward: false });
    expect(r.forwards).toBe(0);
    expect(r.flagged).toBe(false);
    expect(run({ navType: 'back_forward', canGoForward: true }).forwards).toBe(1);
  });
});
