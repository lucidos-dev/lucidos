import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// White blink when an app opens in the iOS PWA.
//
// A freshly mounted app iframe has no document to paint yet, and WKWebView fills
// that gap with the frame's base canvas colour, which is WHITE until something
// inside the frame says otherwise. Everything that would say otherwise arrives
// late and over the network: the app's own `<link>` stylesheet, and the theme
// itself (`/api/v1/sdk-prefs.js` sets `data-theme` + an inline background, and is
// a SECOND request after the HTML). So the sequence on every open is white, then
// themed app, which reads as a flash on a dark theme and is worst on iOS where
// the pane is the whole screen.
//
// None of those gaps is reachable from the host: they are inside a document we
// do not author (apps are user/agent-written HTML and need not include
// sdk-prefs.js at all). The host CAN decide when the frame becomes visible, so it
// covers the frame with an opaque theme surface from mount and crossfades that
// out on the frame's `load`. Then the pane goes app-background to app, with no
// white in between and no dependency on how a given app is written.
//
// The cover is a SIBLING div, deliberately not `opacity: 0` on the iframe itself:
// an app frame WebKit has to re-composite up from fully transparent is the same
// shape as the iOS paint-loss bugs this pane keeps hitting (see
// utils/iosRepaint.ts and ContentPane's resume repaint), and the frame's own
// layer is the one thing we must not make fragile.
//
// The frontend test environment is deliberately non-jsdom, so there is no
// rendered DOM to assert against. Pin the wiring in source, the same approach as
// content-pane-ios-repaint.test.ts.

const here: string = dirname(fileURLToPath(import.meta.url));
const src = readFileSync(resolve(here, 'AppUiInline.tsx'), 'utf-8');
const css = readFileSync(resolve(here, '../../styles/panels/previews.css'), 'utf-8');

describe('app frame load cover', () => {
  it('mounts a cover element alongside the iframe', () => {
    expect(src).toMatch(/app-ui-cover/);
  });

  it('clears the cover on the frame load event, not on a timer alone', () => {
    // `load` is the only signal that the frame has a document to show. A fixed
    // delay would either uncover too early (back to the white flash) or hold a
    // blank pane over an app that was ready.
    expect(src).toMatch(/onLoad=\{\(\) => setLoaded\(true\)\}/);
  });

  it('keeps the cover mounted through its fade-out, at any animation speed', () => {
    // Unmounting on `load` would hard-cut the app in instead of fading it. The
    // fade is a --duration-normal transition, and that token is scaled by the
    // Animation speed slider, so an unscaled linger would unmount the cover
    // partway through its own fade at a slow setting. The slack is a fixed
    // margin, not animation, so it stays outside the scaled term.
    expect(src).toMatch(
      /useLingeringFlag\(!loaded, scaledDurationMs\(COVER_FADE_MS\) \+ COVER_FADE_SLACK_MS\)/,
    );
    expect(src).toMatch(/const COVER_FADE_MS = 200;/);
  });

  it('re-covers the frame on an app switch', () => {
    // An app switch keeps the same iframe element and navigates it
    // (location.replace), so the incoming app reopens the same white-canvas
    // gap the initial mount had. Covering only the mount would leave the
    // flash in place for every open that is not the first one.
    const nav = src.match(/lastSrcRef\.current = src;[\s\S]*?\}, \[src\]\);/)?.[0] ?? '';
    expect(nav).toMatch(/setLoaded\(false\)/);
  });

  it('reveals the frame anyway if load never fires', () => {
    // A request that hangs must not leave the pane covered forever. Whatever
    // the frame painted beats a blank panel.
    expect(src).toMatch(/COVER_MAX_MS/);
    expect(src).toMatch(/setTimeout\(\(\) => setLoaded\(true\), COVER_MAX_MS\)/);
  });

  it('paints the cover with the theme background, opaque and click-through', () => {
    const rule = css.match(/\.app-ui-cover\s*\{[^}]*\}/)?.[0] ?? '';
    expect(rule).toMatch(/background:\s*var\(--bg-primary\)/);
    expect(rule).toMatch(/pointer-events:\s*none/);
    // Opaque until it clears: the cover exists to hide the frame, so anything
    // below 1 would let the white through. Stated explicitly rather than left
    // to the default, matching .thread-skeleton-overlay (the same pattern over
    // the thread scroll area).
    expect(rule).toMatch(/opacity:\s*1/);
  });

  it('positions the cover over the frame', () => {
    expect(css).toMatch(/\.app-ui-inline\s*\{[^}]*position:\s*relative/);
    expect(css).toMatch(/\.app-ui-cover\s*\{[^}]*position:\s*absolute/);
  });
});
