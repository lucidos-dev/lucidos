import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// iOS PWA paint loss on the CONTENT pane.
//
// `.content-pane-body` (styles/panels/shell.css) is a scroll
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

const contentPaneCode = contentPaneSrc
  .replace(/\/\*[\s\S]*?\*\//g, '')
  .replace(/(^|[^:])\/\/.*$/gm, '$1');

describe('ContentPane iOS resume repaint', () => {
  it('subscribes to the shared onPageResume signal', () => {
    expect(contentPaneCode).toMatch(
      /import\s*\{[^}]*\bonPageResume\b[^}]*\}\s*from\s*['"]\.\.\/\.\.\/utils\/pageResume['"]/,
    );
    expect(contentPaneCode).toMatch(/onPageResume\(/);
  });

  it('repaints the content pane body element, not some other node', () => {
    expect(contentPaneCode).toMatch(/forceWebKitRepaint\(\s*bodyRef\.current\s*\)/);
  });

  it('does NOT repaint on panel switch (the reverted lag regression)', () => {
    const repaintCalls = contentPaneCode.match(/forceWebKitRepaint\w*\(/g) ?? [];
    expect(repaintCalls).toHaveLength(1);
    expect(contentPaneCode).toMatch(/onPageResume\([\s\S]*?\)\s*,\s*\[\s*\]\s*\)/);
    for (const block of contentPaneCode.split(/(?=use(?:Layout)?Effect\()/)) {
      const perViewEffect = block.match(/^use(?:Layout)?Effect\([\s\S]*?,\s*\[\s*viewKey\s*\]\s*\)/)?.[0];
      if (!perViewEffect) continue;
      expect(perViewEffect).not.toMatch(/Repaint|bodyRef|scrollTop/);
    }
  });

  it('skips only a pane something is FULLSCREEN over, nothing wider', () => {
    const guards = contentPaneCode.match(/if\s*\(fullscreenCoversThePane\(\)\)\s*return/g) ?? [];
    expect(guards).toHaveLength(1);
  });

  it('does not skip an ordinary in-pane app', () => {
    const predicate = contentPaneCode.match(/function fullscreenCoversThePane\(\)[\s\S]*?\n}/)?.[0] ?? '';
    expect(predicate).not.toMatch(/panelOverlay|'app-ui'/);
  });

  it('asks the DOM about native fullscreen, not the app-panel overlay mount', () => {
    expect(contentPaneCode).toMatch(/nativeFullscreenElement\(\)\s*!==\s*null/);
    expect(contentPaneCode).not.toMatch(/appFullscreenHost\.peek/);
  });

  it('reads the fullscreen state live, not at render time', () => {
    expect(contentPaneCode).toMatch(/appPseudoFullscreen\.peek\(\)/);
  });

  it('uses the shared repaint utilities rather than a hand-rolled toggle', () => {
    expect(contentPaneCode).toMatch(
      /import\s*\{[^}]*\bforceWebKitRepaint\b[^}]*\}\s*from\s*['"]\.\.\/\.\.\/utils\/webkitRepaint['"]/,
    );
    expect(contentPaneCode).not.toMatch(/style\.transform/);
  });
});
