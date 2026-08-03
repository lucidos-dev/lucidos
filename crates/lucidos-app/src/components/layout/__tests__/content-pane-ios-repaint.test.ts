import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// iOS PWA paint loss on the CONTENT pane.
//
// `.content-pane-body` (styles/panels/shell.css) is an `overflow-y: auto` scroll
// container, so WKWebView gives it its own compositing layer. While the PWA is
// backgrounded (the phone locked) WebKit stops committing the layer tree and that
// layer freezes on a stale-or-empty backing texture: the panel is fully built and
// laid out in the DOM, and nothing is on screen. This is exactly the blank
// root-caused for `.thread-content` by the `ios-pwa-blackout` investigation
// (24/24 render probes classified `content-present`; see
// docs/temporary-measures.md), but the repaint hardening that closed it was wired
// into ThreadView / CreateThreadView only. The content pane never got it.
//
// The recovery is a repaint on RESUME, and only on resume. No signal changes on
// wake (same panel, same data), so no render produces DOM changes and only an
// explicit repaint can un-blank the layer. The shared `onPageResume` signal covers
// all three iOS wake events (visibilitychange / pageshow / focus), which is also
// why one wake gets three superseding attempts: iOS often restores a PWA through
// `pageshow` with no `visible` visibilitychange.
//
// The element is LONG-LIVED, with every view swapping children inside it, which is
// why the blank read as unrecoverable before the fix: tapping Notifications
// rebuilt the DOM inside a layer the compositor had stopped committing, and only a
// reload built a new one. That made a per-view repaint tempting, and it is
// deliberately NOT done. See the `does NOT repaint on panel switch` case below for
// what it cost.
//
// A compositor paint loss is INVISIBLE to JS (the DOM is present, laid out and
// non-zero height; only the texture is stale), and the frontend test environment
// is deliberately non-jsdom, so there is no observable state to assert on. Pin
// the wiring in source instead, the same approach as the `ThreadView resume
// repaint wiring` guard in utils/pageResume.test.ts and the wake-recovery guard
// in mobile-swipe-app-height-recovery.test.ts.

const here: string = dirname(fileURLToPath(import.meta.url));
const contentPaneSrc = readFileSync(resolve(here, '../ContentPane.tsx'), 'utf-8');

/** The same source with comments stripped. The negative assertions below ban a
 *  CALL, not the word, and this component's comments deliberately explain the
 *  reverted per-view repaint: naming `forceIOSRepaintBurst` or `[viewKey]` while
 *  explaining why they are gone must not turn the guard red. Scanning raw source
 *  would make that prose fail the test it exists to document. */
const contentPaneCode = contentPaneSrc
  .replace(/\/\*[\s\S]*?\*\//g, '')
  .replace(/(^|[^:])\/\/.*$/gm, '$1');

describe('ContentPane iOS resume repaint', () => {
  it('subscribes to the shared onPageResume signal', () => {
    // Not a bare `visibilitychange` listener: iOS frequently restores a PWA via
    // `pageshow` (bfcache) or `focus` with no `visible` visibilitychange, and
    // `onPageResume` is the single place all three are handled (and the wake-tap
    // guard armed).
    expect(contentPaneCode).toMatch(
      /import\s*\{[^}]*\bonPageResume\b[^}]*\}\s*from\s*['"]\.\.\/\.\.\/utils\/pageResume['"]/,
    );
    expect(contentPaneCode).toMatch(/onPageResume\(/);
  });

  it('repaints the content pane body element, not some other node', () => {
    // The blanked layer is the scroll container itself, and `bodyRef` is the only
    // ref pointing at `.content-pane-body`.
    expect(contentPaneCode).toMatch(/forceIOSRepaint\(\s*bodyRef\.current\s*\)/);
  });

  it('does NOT repaint on panel switch (the reverted lag regression)', () => {
    // A per-view repaint keyed on `viewKey` was tried and reverted.
    // `forceIOSRepaint`'s recovery nudge writes `scrollTop`, and `useHideOnScroll`
    // listens for scroll on this exact element
    // (`.mobile-swipe-pane .content-pane-body`), so every nudge moved the mobile
    // header and rewrote `--mobile-header-offset` on `:root`. As a 5-attempt burst
    // that came to ten header transforms plus five forced synchronous layouts per
    // view change, while the incoming view was still mounting its lazy chunk and
    // its data. The notification detail keys `viewKey` per notification, so each
    // prev/next chevron tap paid it too. Reported as lag opening notifications.
    //
    // As of 2026-08-03 that cost is gone: `useHideOnScroll` skips scroll events
    // inside the nudge window (`isRepaintNudging`), and the offset is written on
    // its two consumer elements as a `transform` rather than on `:root` as a
    // `top`, so a nudge no longer moves the header or forces a layout.
    //
    // This test still holds, on the reason that outlived the regression: the wake
    // already fires visibilitychange + pageshow + focus, so the resume path gets
    // three superseding attempts on its own and needs no navigation fallback.
    //
    // Stated as "exactly one repaint call site, and it is inside a mount-once
    // effect". That covers the burst, a second plain toggle, and any future
    // variant, without depending on the banned symbol's name.
    const repaintCalls = contentPaneCode.match(/forceIOSRepaint\w*\(/g) ?? [];
    expect(repaintCalls).toHaveLength(1);
    expect(contentPaneCode).toMatch(/onPageResume\([\s\S]*?\)\s*,\s*\[\s*\]\s*\)/);
    expect(contentPaneCode).not.toMatch(/\[\s*viewKey\s*\]/);
  });

  it('skips the app-ui overlay on the repaint path', () => {
    // `forceIOSRepaint` writes a transform for one frame, which makes
    // `.content-pane-body` the containing block for the pseudo-fullscreen app
    // panel's `position: fixed` (`.app-ui-fullscreen`, rendered in-tree by
    // AppUiInline), snapping a fullscreen app back to the pane's box for that
    // frame. It also buys nothing there: that body is `overflow: hidden` around an
    // iframe, so it is not the scroll container that blanks. One guard for the one
    // repaint path; a second path would need its own.
    const guards = contentPaneCode.match(/if\s*\(hostsAppUiIframe\(\)\)\s*return/g) ?? [];
    expect(guards).toHaveLength(1);
  });

  it('reads the overlay live in the guard, not the render-time isAppUi', () => {
    // The resume subscription is mounted once with `[]` deps, so a captured
    // `isAppUi` would be frozen at whatever the pane showed at mount and the guard
    // would consult the wrong overlay forever after.
    expect(contentPaneCode).toMatch(/function hostsAppUiIframe\(\)[\s\S]{0,120}?panelOverlay\.peek\(\)/);
  });

  it('uses the shared repaint utilities rather than a hand-rolled toggle', () => {
    // `forceIOSRepaint` is iOS-gated, detached-node-safe, supersede-safe, and
    // yields to concurrent scroll writers (useScrollMemory's restore). A local
    // `style.transform` poke would have none of that and would fight the
    // saved-scroll restore this same component sets up.
    expect(contentPaneCode).toMatch(
      /import\s*\{[^}]*\bforceIOSRepaint\b[^}]*\}\s*from\s*['"]\.\.\/\.\.\/utils\/iosRepaint['"]/,
    );
    expect(contentPaneCode).not.toMatch(/style\.transform/);
  });
});
