/**
 * The mobile thread title bar undoes the iOS repaint nudge's compensation.
 *
 * `forceIOSRepaint` (utils/iosRepaint.ts) recovers a frozen WKWebView layer. It
 * writes 1px of `scrollTop` on `.thread-content`, then cancels the painted
 * motion with a `translateY` on that same element. The cancellation is exact for
 * content the scroll moved. `.mobile-thread-title-row` is `position: sticky`, so
 * the scroll moved it not at all and the transform reaches it uncancelled. It
 * flicked a pixel per toggle: five on every thread open, and a shimmer while a
 * reply streamed.
 *
 * The contract has three halves in three files, and each alone is silent. The
 * primitive publishes its shift onto a marked DIRECT child. The row carries the
 * marker and is such a child. The CSS rule subtracts what was published. Break
 * any one and the property reads its `0px` fallback. All three fail here.
 *
 * A source scan, because nothing automated reaches the real thing: the nudge is
 * `isIOS()`-gated and Playwright's WebKit is not the iOS PWA. Plan:
 * docs/plans/2026-08-16-the-sticky-title-undoes-the-repaint-nudge-compensation.md
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { rulesTargeting, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

const primitive = read('../../utils/iosRepaint.ts');
const header = read('../../components/layout/MobileAppHeader.tsx');
const threadView = read('../../components/chat/ThreadView.tsx');

/** Every stylesheet the app ships, keyed by path. The whole tree rather than the
 *  one rule's own file: a `transform` set on this row from any sheet drops the
 *  counter, and reading one file would call that clean. */
const sheets: Array<[string, string]> = readdirSync(resolve(here, '../..'), { recursive: true })
  .filter((p: unknown): p is string => typeof p === 'string' && p.endsWith('.css'))
  .map((p: string) => [p, readFileSync(resolve(here, '../..', p), 'utf-8')] as [string, string]);

/** The value of a `const NAME = '…'` in the primitive. The consumers are checked
 *  against the spelling the code publishes, not against a copy that can drift. */
function literal(name: string): string {
  const m = new RegExp(`${name}\\s*=\\s*'([^']+)'`).exec(primitive);
  expect(m, `${name} not declared in utils/iosRepaint.ts`).not.toBeNull();
  return m![1];
}

const SHIFT_PROP = literal('PINNED_SHIFT_PROP');
const PINNED_ATTR = literal('SCROLLER_PINNED_ATTR');

/** Every rule in the app that styles the title row itself and sets a
 *  `transform`, tagged with the sheet it came from. */
function transformRules(): Array<[string, CssRule]> {
  return sheets.flatMap(([path, css]) =>
    rulesTargeting(css, 'mobile-thread-title-row')
      .filter(rule => rule.props.has('transform'))
      .map(rule => [path, rule] as [string, CssRule]),
  );
}

describe('the sticky thread title undoes the repaint nudge', () => {
  it('subtracts the published shift in every transform set on the row', () => {
    const rules = transformRules();
    expect(rules.length).toBeGreaterThan(0);
    for (const [path, rule] of rules) {
      expect(rule.props.get('transform'), `${path}: ${rule.selector}`).toContain(`- var(${SHIFT_PROP}`);
    }
  });

  it('keeps the hide-on-scroll offset term in the same transform', () => {
    // The counter is a second term, not a replacement: this is the one the
    // header's hide-on-scroll drives, and the row rides it out of view on it.
    for (const [path, rule] of transformRules()) {
      expect(rule.props.get('transform'), `${path}: ${rule.selector}`).toContain('var(--mobile-header-offset');
    }
  });

  it('marks the row so the primitive can find it', () => {
    const tag = /<div\b[^>]*?mobile-thread-title-row[^>]*>/.exec(header);
    expect(tag, 'no opening tag carrying .mobile-thread-title-row').not.toBeNull();
    expect(tag![0]).toContain(PINNED_ATTR);
  });

  it('renders the row as a direct child of the scroller the nudge writes to', () => {
    // The publish scans the scroller's own children, so wrapping the row one
    // level deeper would stop it and leave the CSS on its 0px fallback. The
    // scroller is the element `areaRef` points at: what `forceIOSRepaint` gets.
    const open = /<div[^>]*class="thread-content visible"[^>]*ref=\{areaRef\}[^>]*>/.exec(threadView);
    expect(open, 'no transcript container carrying ref={areaRef}').not.toBeNull();
    const after = threadView.slice(open!.index + open![0].length);
    const at = after.indexOf('<MobileThreadTitleBar');
    expect(at, 'MobileThreadTitleBar is not rendered inside the transcript').toBeGreaterThanOrEqual(0);
    expect(after.slice(0, at), 'a wrapper stands between the scroller and the row').not.toContain('<');
  });

  it('publishes the counter on that child alone, never on :root', () => {
    // A custom property inherits. Declaring it on the root would put every
    // transcript node's style back in play on each nudge.
    for (const [path, css] of sheets) {
      expect(css, path).not.toMatch(new RegExp(`:root[^{]*\\{[^}]*${SHIFT_PROP}`));
    }
  });
});
