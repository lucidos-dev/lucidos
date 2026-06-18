/**
 * Regression test for "Link to <app> goes to index.html preview instead of app".
 *
 * The user reported this on iOS PWA: tapping a markdown link of the form
 * `[Name](apps/<id>/index.html)` in a chat response opened the file preview
 * (or fell through to a 404/SPA fallback) instead of opening the running
 * app.
 *
 * Two layers fix it (both gated by this test):
 *   1. linkifyPaths.rewriteAppAnchor — turns `<a href="apps/<id>/index.html">`
 *      into `<a href="#" class="app-link" data-app-id="<id>">` at render
 *      time. Verified directly by the linkifyPaths.test.ts suite.
 *   2. ChatExchange.handleLinkClick fallback — intercepts a click on ANY
 *      anchor whose href is `apps/<id>/...` even when the rewriter didn't
 *      run (stale memo, iOS PWA bundle predating the rewriter, apps list
 *      not loaded at first render). Verified here.
 *
 * The codebase deliberately ships no DOM library in tests, so we use small
 * mock element objects that implement just the `closest()` / `getAttribute()`
 * / `dataset` surface the handler touches.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import { linkifyPaths, extractAppIdFromHref, extractNavTargetFromHref } from '../../../utils/linkifyPaths';
import { renderMarkdown } from '../../../utils/renderMarkdown';
import type { App } from '../../../store/types';

const here: string = dirname(fileURLToPath(import.meta.url));
const chatExchangeSource = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

const APPS: App[] = [
  { id: 'work-tracker', name: 'Lucidos Work', description: 'x' },
  { id: 'momentum-autoresearch', name: 'Momentum Autoresearch', description: 'y' },
];

interface MockAnchor {
  tagName: 'A';
  href: string;
  className: string;
  dataset: Record<string, string>;
  getAttribute(name: string): string | null;
  closest(selector: string): MockAnchor | null;
}

function mkAnchor(href: string, className = '', dataAttrs: Record<string, string> = {}): MockAnchor {
  const classes = className ? className.split(/\s+/) : [];
  const el: MockAnchor = {
    tagName: 'A',
    href,
    className,
    dataset: dataAttrs,
    getAttribute(name: string): string | null {
      if (name === 'href') return href;
      if (name === 'class') return className;
      return null;
    },
    closest(selector: string): MockAnchor | null {
      // Tag-name selector
      if (selector === 'a') return el;
      // Class selector
      if (selector.startsWith('.')) {
        const cls = selector.slice(1);
        return classes.includes(cls) ? el : null;
      }
      return null;
    },
  };
  return el;
}

function mkEvent(target: MockAnchor): { target: MockAnchor; defaultPrevented: boolean; preventDefault: () => void } {
  const e = {
    target,
    defaultPrevented: false,
    preventDefault() { e.defaultPrevented = true; },
  };
  return e;
}

/** Mirror of handleLinkClick's branch order from ChatExchange.tsx. Pinned
 *  by the source-regex test below — any structural change in the real
 *  handler must also update this mirror, and the regex assertion will
 *  catch a divergence. */
type Callbacks = {
  openImage: (src: string, target: any) => void;
  openArtifact: (path: string) => void;
  openApp: (app: App) => void;
  navigate: (req: { target: string }) => void;
};
function runHandleLinkClick(e: ReturnType<typeof mkEvent>, apps: App[], cb: Callbacks): void {
  const t = e.target;
  const img = t.closest('.image-thumbnail');
  if (img) { e.preventDefault(); cb.openImage((img as any).dataset.fullSrc || (img as any).href, img); return; }
  const art = t.closest('.artifact-link');
  if (art) { e.preventDefault(); const p = (art as any).dataset.path; if (p) cb.openArtifact(p); return; }
  const app = t.closest('.app-link');
  if (app) {
    e.preventDefault();
    const id = (app as any).dataset.appId;
    if (id) { const a = apps.find(x => x.id === id); if (a) cb.openApp(a); }
    return;
  }
  const nav = t.closest('.nav-link');
  if (nav) {
    e.preventDefault();
    const target = (nav as any).dataset.navTarget;
    if (target) cb.navigate({ target });
    return;
  }
  // Defense-in-depth fallback
  const anchor = t.closest('a');
  if (anchor) {
    const href = anchor.getAttribute('href') || '';
    const id = extractAppIdFromHref(href);
    if (id) {
      const a = apps.find(x => x.id === id);
      if (a) { e.preventDefault(); cb.openApp(a); return; }
    }
    const navName = extractNavTargetFromHref(href);
    if (navName) {
      e.preventDefault();
      cb.navigate({ target: navName });
      return;
    }
  }
}

describe('chat link click — the bug-report scenario', () => {
  let cb: Callbacks & {
    openImage: ReturnType<typeof vi.fn>;
    openArtifact: ReturnType<typeof vi.fn>;
    openApp: ReturnType<typeof vi.fn>;
    navigate: ReturnType<typeof vi.fn>;
  };

  beforeEach(() => {
    cb = {
      openImage: vi.fn() as Callbacks['openImage'] & ReturnType<typeof vi.fn>,
      openArtifact: vi.fn() as Callbacks['openArtifact'] & ReturnType<typeof vi.fn>,
      openApp: vi.fn() as Callbacks['openApp'] & ReturnType<typeof vi.fn>,
      navigate: vi.fn() as Callbacks['navigate'] & ReturnType<typeof vi.fn>,
    };
  });

  it('PRIMARY: pre-rewritten <a class="app-link"> click → openApp', () => {
    const a = mkAnchor('#', 'app-link', { appId: 'work-tracker' });
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).toHaveBeenCalledWith(APPS[0]);
    expect(e.defaultPrevented).toBe(true);
  });

  it('PRIMARY: .app-link with unknown id → preventDefault but no openApp', () => {
    // Real handler unconditionally preventDefaults inside the .app-link
    // branch even when the id doesn't resolve, to avoid a stale anchor
    // navigating to "#" and scrolling to top. The mirror mirrors that.
    const a = mkAnchor('#', 'app-link', { appId: 'unknown-app' });
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });

  it('FALLBACK: plain <a href="apps/<id>/index.html"> click → openApp', () => {
    // The shape that survives if linkifyPaths didn't rewrite.
    const a = mkAnchor('apps/work-tracker/index.html');
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).toHaveBeenCalledWith(APPS[0]);
    expect(e.defaultPrevented).toBe(true);
  });

  it.each([
    '/apps/work-tracker/index.html',
    'data/apps/work-tracker/index.html',
    '/data/apps/work-tracker/index.html',
    'apps/work-tracker',
    'apps/work-tracker/',
    'apps/work-tracker/index.html?v=2',
    'apps/work-tracker/index.html#section',
    // `app:<id>` custom-scheme shorthand. The Momentum-app bug report:
    // LLM wrote `[Momentum app](app:momentum)`, which fell through to the
    // browser and dead-ended on macOS Chrome.
    'app:work-tracker',
    'app:work-tracker/',
    'app:work-tracker?refresh=1',
    'app:work-tracker#section',
  ])('FALLBACK entry-point: %s → openApp', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).toHaveBeenCalledWith(APPS[0]);
    expect(e.defaultPrevented).toBe(true);
  });

  it.each([
    'apps/work-tracker/styles.css',
    'apps/work-tracker/scripts/run.sh',
    'apps/work-tracker/nested/deep/file.json',
  ])('FALLBACK sub-file: %s → does NOT intercept (sub-file should preview as artifact)', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  it('does NOT intercept unknown app id — default navigation proceeds', () => {
    const e = mkEvent(mkAnchor('apps/no-such-app/index.html'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  it('does NOT intercept external https URLs that happen to contain apps/', () => {
    const e = mkEvent(mkAnchor('https://example.com/apps/work-tracker/index.html'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  it('END-TO-END: render → linkify pipeline yields the .app-link the click expects', () => {
    // The exact markdown shape the LLM wrote in the bug-report thread.
    const md = 'Open it in [Lucidos Work](apps/work-tracker/index.html).';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="app-link"');
    expect(html).toContain('data-app-id="work-tracker"');
    expect(html).toContain('>Lucidos Work</a>');
  });

  it('END-TO-END: render → linkify pipeline rewrites app:<id> custom scheme', () => {
    // Exact markdown from the Momentum-app bug-report thread:
    //   Open the [Momentum app](app:momentum) and switch to the Backtest tab.
    const md = 'Open the [Momentum Autoresearch](app:momentum-autoresearch) and switch to the Backtest tab.';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="app-link"');
    expect(html).toContain('data-app-id="momentum-autoresearch"');
    expect(html).toContain('>Momentum Autoresearch</a>');
    expect(html).not.toContain('href="app:');
  });

  // ---------------------------------------------------------------------------
  // nav-link bug report — `[Notifications](data/notifications)` was a dead link.
  // The LLM naturally writes `data/<panel-name>` mirroring the artifact/app
  // shape; without rewrite + click routing the browser hits the engine's
  // /data/* static mount and 404s.
  // ---------------------------------------------------------------------------

  it('PRIMARY: pre-rewritten <a class="nav-link"> click → handleNavigationRequest', () => {
    const a = mkAnchor('#', 'nav-link', { navTarget: 'notifications' });
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).toHaveBeenCalledWith({ target: 'notifications' });
    expect(e.defaultPrevented).toBe(true);
  });

  it('FALLBACK: plain <a href="data/notifications"> click → handleNavigationRequest', () => {
    const a = mkAnchor('data/notifications');
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).toHaveBeenCalledWith({ target: 'notifications' });
    expect(e.defaultPrevented).toBe(true);
  });

  it.each([
    ['notifications', 'notifications'],
    ['/notifications', 'notifications'],
    ['data/notifications', 'notifications'],
    ['/data/notifications', 'notifications'],
    ['notifications/', 'notifications'],
    ['notifications?refresh=1', 'notifications'],
    ['apps', 'apps'],
    ['app-store', 'app-store'],
    ['triggers', 'triggers'],
    ['changes', 'changes'],
    ['files', 'files'],
    ['settings', 'settings'],
  ])('FALLBACK panel: %s → navigate(target=%s)', (href, target) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).toHaveBeenCalledWith({ target });
    expect(e.defaultPrevented).toBe(true);
  });

  it('does NOT intercept unknown panel name — default navigation proceeds', () => {
    const e = mkEvent(mkAnchor('unknown-panel'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  it('does NOT intercept external https URLs that happen to contain a panel name', () => {
    const e = mkEvent(mkAnchor('https://example.com/notifications'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  it('END-TO-END: render → linkify yields the .nav-link the click expects (bug-report shape)', () => {
    // Exact markdown from the bug-report thread:
    //   Open it: [Notifications](data/notifications) or [Momentum …](apps/momentum-autoresearch/index.html).
    const md = 'Open it: [Notifications](data/notifications).';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="nav-link"');
    expect(html).toContain('data-nav-target="notifications"');
    expect(html).toContain('>Notifications</a>');
  });
});

describe('chat link click — handler structure pin', () => {
  // Catches a future edit that quietly changes the branch structure of the
  // real handleLinkClick in ChatExchange.tsx, which would let the in-test
  // mirror (runHandleLinkClick above) drift and give us false confidence.
  it('handleLinkClick has the five branches in the documented order, each terminated by return;', () => {
    // The five branches must appear in this order: image → artifact → app
    // → nav → anchor-fallback. Each branch must close with `return;` before
    // the next `.closest(...)` opens — otherwise a refactor that swaps two
    // if-bodies (e.g. matches the targets in one order but acts on them
    // in another) would slip past a simpler text-order pin. The lazy
    // quantifier guarantees the first `return;` after each selector is the
    // boundary, so reordering forces a regex break.
    const m = chatExchangeSource.match(/function handleLinkClick[\s\S]*?\n  \}\n/);
    expect(m, 'handleLinkClick not found in ChatExchange.tsx').not.toBeNull();
    const body = m![0];
    const sequence =
      /closest\('\.image-thumbnail'\)[\s\S]+?return;[\s\S]+?closest\('\.artifact-link'\)[\s\S]+?return;[\s\S]+?closest\('\.app-link'\)[\s\S]+?return;[\s\S]+?closest\('\.nav-link'\)[\s\S]+?return;[\s\S]+?closest\('a'\)/;
    expect(body).toMatch(sequence);
  });

  it('handleLinkClick uses extractAppIdFromHref and extractNavTargetFromHref in the fallback branch', () => {
    expect(chatExchangeSource).toMatch(/import.*extractAppIdFromHref.*extractNavTargetFromHref.*from.*linkifyPaths/);
    expect(chatExchangeSource).toMatch(/extractAppIdFromHref\(rawHref\)/);
    expect(chatExchangeSource).toMatch(/extractNavTargetFromHref\(rawHref\)/);
  });

  it('fallback branch calls openApp and handleNavigationRequest with preventDefault', () => {
    const m = chatExchangeSource.match(/closest\('a'\)[\s\S]*?\n  \}\n/);
    expect(m).not.toBeNull();
    const body = m![0];
    expect(body).toContain('openApp(app)');
    expect(body).toContain('handleNavigationRequest({ target: navName })');
    expect(body).toContain('e.preventDefault()');
  });
});
