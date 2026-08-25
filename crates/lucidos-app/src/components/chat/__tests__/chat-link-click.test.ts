// @vitest-environment jsdom
// The sanitizer runs on a real DOM. The default `node` environment has none,
// and DOMPurify would pass its input straight back.

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
import { linkifyPaths, extractAppIdFromHref, extractNavTargetFromHref, extractLocalFileTarget, extractBareAppRef, extractDataPathTarget, extractTriggerIdFromHref, hasUrlScheme, browserHandlesHref } from '../../../utils/linkifyPaths';
import { renderMarkdown } from '../../../utils/renderMarkdown';
import type { App } from '../../../store/types';

const here: string = dirname(fileURLToPath(import.meta.url));
const chatExchangeSource = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

const APPS: App[] = [
  { id: 'work-tracker', name: 'Lucidos Work', description: 'x' },
  { id: 'habit-tracker', name: 'Habit Tracker', description: 'y' },
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
  openTrigger: (id: string) => void;
  navigate: (req: { target: string }) => void;
  osOpen: (target: string) => void;
  toast: (message: string) => void;
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
  const trig = t.closest('.trigger-link');
  if (trig) {
    e.preventDefault();
    const triggerId = (trig as any).dataset.triggerId;
    if (triggerId) cb.openTrigger(triggerId);
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
    const triggerId = extractTriggerIdFromHref(href);
    if (triggerId) {
      e.preventDefault();
      cb.openTrigger(triggerId);
      return;
    }
    const navName = extractNavTargetFromHref(href);
    if (navName) {
      e.preventDefault();
      cb.navigate({ target: navName });
      return;
    }
    const bareRef = extractBareAppRef(href);
    if (bareRef) {
      const a = apps.find(x => x.id === bareRef || x.name === bareRef);
      if (a) { e.preventDefault(); cb.openApp(a); return; }
    }
    const dataPath = extractDataPathTarget(href);
    if (dataPath) {
      e.preventDefault();
      cb.openArtifact(dataPath);
      return;
    }
    const localFile = extractLocalFileTarget(href);
    if (localFile) {
      e.preventDefault();
      cb.osOpen(localFile);
      return;
    }
    // Terminal guard: an href the browser cannot act on reaches nothing, so
    // it must never navigate. Mirrors ChatExchange's `deadLinkMessage`.
    if (!browserHandlesHref(href) && !href.startsWith('#')) {
      e.preventDefault();
      if (!href) cb.toast('This link has no destination');
      else if (hasUrlScheme(href)) cb.toast(`Link "${href}" uses a scheme nothing here can open`);
      else cb.toast(`Link "${href}" points nowhere in this workspace`);
    }
  }
}

describe('chat link click — the bug-report scenario', () => {
  let cb: Callbacks & {
    openImage: ReturnType<typeof vi.fn>;
    openArtifact: ReturnType<typeof vi.fn>;
    openApp: ReturnType<typeof vi.fn>;
    openTrigger: ReturnType<typeof vi.fn>;
    navigate: ReturnType<typeof vi.fn>;
    osOpen: ReturnType<typeof vi.fn>;
    toast: ReturnType<typeof vi.fn>;
  };

  beforeEach(() => {
    cb = {
      openImage: vi.fn() as Callbacks['openImage'] & ReturnType<typeof vi.fn>,
      openArtifact: vi.fn() as Callbacks['openArtifact'] & ReturnType<typeof vi.fn>,
      openApp: vi.fn() as Callbacks['openApp'] & ReturnType<typeof vi.fn>,
      openTrigger: vi.fn() as Callbacks['openTrigger'] & ReturnType<typeof vi.fn>,
      navigate: vi.fn() as Callbacks['navigate'] & ReturnType<typeof vi.fn>,
      osOpen: vi.fn() as Callbacks['osOpen'] & ReturnType<typeof vi.fn>,
      toast: vi.fn() as Callbacks['toast'] & ReturnType<typeof vi.fn>,
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
    // `app:<id>` custom-scheme shorthand. The Habit Tracker-app bug report:
    // LLM wrote `[Habit Tracker app](app:habit-tracker)`, which fell through to the
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
  ])('FALLBACK sub-file: %s → previews as artifact, never opens the app', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(cb.openArtifact).toHaveBeenCalledWith(href);
    expect(e.defaultPrevented).toBe(true);
  });

  it('unknown app id → previews the file, never navigates away', () => {
    const e = mkEvent(mkAnchor('apps/no-such-app/index.html'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(cb.openArtifact).toHaveBeenCalledWith('apps/no-such-app/index.html');
    expect(e.defaultPrevented).toBe(true);
  });

  it('does NOT intercept external https URLs that happen to contain apps/', () => {
    const e = mkEvent(mkAnchor('https://example.com/apps/work-tracker/index.html'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  // ---------------------------------------------------------------------------
  // bare app-id/name href — the reported bug. The LLM wrote a link with the app
  // id as a bare relative href — `[Habit Tracker](habit-tracker)`, no apps/ prefix,
  // no app: scheme — mirroring `[Notifications](notifications)`. Left alone the
  // browser navigates to the relative href and the SPA fallback reloads the whole
  // workspace (the "Opening workspace" splash on iOS PWA).
  // ---------------------------------------------------------------------------

  it.each([
    'work-tracker',      // bare id
    '/work-tracker',     // leading slash
    'work-tracker/',     // trailing slash
    'work-tracker?v=2',  // query
    'work-tracker#top',  // fragment
  ])('BARE app-id href %s → openApp', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).toHaveBeenCalledWith(APPS[0]);
    expect(e.defaultPrevented).toBe(true);
  });

  it('BARE app-NAME href, percent-encoded (Habit%20Tracker) → openApp', () => {
    // Markdown renders a spaced destination encoded, so the real DOM href is
    // `Habit%20Tracker`; extractBareAppRef decodes it back to the raw name.
    const e = mkEvent(mkAnchor('Habit%20Tracker'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).toHaveBeenCalledWith(APPS[1]);
    expect(e.defaultPrevented).toBe(true);
  });

  it('a bare href that names no known app is swallowed, not navigated', () => {
    const e = mkEvent(mkAnchor('README'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(cb.toast).toHaveBeenCalledOnce();
    expect(e.defaultPrevented).toBe(true);
  });

  it('a bare href that is a nav panel name routes to the panel, not a bare app', () => {
    // nav check runs before the bare-app-ref branch, so `notifications` keeps
    // routing to its panel even though it's a bare single-segment href.
    const e = mkEvent(mkAnchor('notifications'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).toHaveBeenCalledWith({ target: 'notifications' });
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });

  it('END-TO-END: render → linkify rewrites the bare app-id href (reported bug shape)', () => {
    const md = 'Open [Lucidos Work](work-tracker) for details.';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="app-link"');
    expect(html).toContain('data-app-id="work-tracker"');
    expect(html).toContain('>Lucidos Work</a>');
    expect(html).not.toContain('href="work-tracker"');
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
    // Exact markdown from the Habit Tracker-app bug-report thread:
    //   Open the [Habit Tracker app](app:habit-tracker) and switch to the Backtest tab.
    const md = 'Open the [Habit Tracker](app:habit-tracker) and switch to the Backtest tab.';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="app-link"');
    expect(html).toContain('data-app-id="habit-tracker"');
    expect(html).toContain('>Habit Tracker</a>');
    expect(html).not.toContain('href="app:');
  });

  // ---------------------------------------------------------------------------
  // nav-link bug report — `[Notifications](data/notifications)` was a dead link.
  // The LLM naturally writes `data/<panel-name>` mirroring the artifact/app
  // shape; without rewrite + click routing the browser hits the engine's
  // /data/* static mount and 404s.
  // ---------------------------------------------------------------------------

  // ---------------------------------------------------------------------------
  // Trigger deep links. The reported bug: told to link the trigger, the agent
  // wrote `[name](trigger:<uuid>)`. Nothing claimed the href, and the terminal
  // guard exempts anything carrying a scheme. The browser has no handler for
  // `trigger:`, so the click did nothing at all, silently.
  // ---------------------------------------------------------------------------

  it('PRIMARY: pre-rewritten <a class="trigger-link"> click → navigateToTrigger', () => {
    const a = mkAnchor('#', 'trigger-link', { triggerId: '3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63' });
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openTrigger).toHaveBeenCalledWith('3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63');
    expect(e.defaultPrevented).toBe(true);
  });

  it('FALLBACK: plain <a href="trigger:<id>"> click → navigateToTrigger, never the browser', () => {
    const e = mkEvent(mkAnchor('trigger:3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openTrigger).toHaveBeenCalledWith('3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63');
    expect(e.defaultPrevented).toBe(true);
  });

  it('the triggers PANEL still routes to the panel, not to a trigger', () => {
    const e = mkEvent(mkAnchor('triggers'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).toHaveBeenCalledWith({ target: 'triggers' });
    expect(cb.openTrigger).not.toHaveBeenCalled();
  });

  it('END-TO-END: render → linkify yields the .trigger-link the click expects', () => {
    // Exact markdown from the bug-report thread.
    const md = 'Here it is: [Nightly digest](trigger:3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63)';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="trigger-link"');
    expect(html).toContain('data-trigger-id="3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63"');
    expect(html).toContain('>Nightly digest</a>');
  });

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

  it('an unknown panel name is swallowed, not navigated', () => {
    const e = mkEvent(mkAnchor('unknown-panel'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).not.toHaveBeenCalled();
    expect(cb.toast).toHaveBeenCalledOnce();
    expect(e.defaultPrevented).toBe(true);
  });

  it('does NOT intercept external https URLs that happen to contain a panel name', () => {
    const e = mkEvent(mkAnchor('https://example.com/notifications'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  it('END-TO-END: render → linkify yields the .nav-link the click expects (bug-report shape)', () => {
    // Exact markdown from the bug-report thread:
    //   Open it: [Notifications](data/notifications) or [Habit Tracker …](apps/habit-tracker/index.html).
    const md = 'Open it: [Notifications](data/notifications).';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="nav-link"');
    expect(html).toContain('data-nav-target="notifications"');
    expect(html).toContain('>Notifications</a>');
  });

  // ---------------------------------------------------------------------------
  // file:// + absolute-path bug report — the release flow hands the user a
  // clickable link to a staged .dmg that lives OUTSIDE the workspace (under
  // ~/…/.lucidos/release-worktrees/<version>/…). Those hrefs must open with the
  // OS (mount the dmg / reveal the folder), NOT route through the in-app file
  // preview, openApp, handleNavigationRequest, or the /data/* static mount.
  // ---------------------------------------------------------------------------

  it('OS-OPEN: file:///abs/path.dmg → osOpen, not navigate / openApp / openArtifact', () => {
    const href = 'file:///Users/me/.lucidos/release-worktrees/0.12.3/Lucidos_0.12.3_aarch64.dmg';
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.osOpen).toHaveBeenCalledWith(href);
    expect(cb.navigate).not.toHaveBeenCalled();
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(cb.openArtifact).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });

  it('OS-OPEN: bare absolute path /Users/.../x.dmg → osOpen, not navigate / openArtifact', () => {
    const href = '/Users/me/Downloads/Lucidos_0.12.3_aarch64.dmg';
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.osOpen).toHaveBeenCalledWith(href);
    expect(cb.navigate).not.toHaveBeenCalled();
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(cb.openArtifact).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });

  it('OS-OPEN: absolute folder path is revealed via the OS', () => {
    const href = '/Users/me/.lucidos/release-worktrees/0.12.3';
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.osOpen).toHaveBeenCalledWith(href);
    expect(e.defaultPrevented).toBe(true);
  });

  it.each([
    // Absolute workspace routes are claimed by the app/nav extractors BEFORE
    // the OS-open branch — they must never be handed to the OS as disk paths.
    '/data/artifacts/report.pdf',
    '/data',
    '/apps/work-tracker/styles.css',
    '/apps',
  ])('OS-OPEN: workspace absolute route %s is NOT OS-opened', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.osOpen).not.toHaveBeenCalled();
  });

  it('OS-OPEN: an absolute /apps/<id>/index.html still opens the app (not OS-open)', () => {
    // Regression guard: the app extractor runs first, so an entry-point under
    // an absolute /apps/ path routes to openApp, never to the OS opener.
    const e = mkEvent(mkAnchor('/apps/work-tracker/index.html'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).toHaveBeenCalledWith(APPS[0]);
    expect(cb.osOpen).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });

  it('OS-OPEN: an absolute /notifications still navigates the panel (not OS-open)', () => {
    const e = mkEvent(mkAnchor('/notifications'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).toHaveBeenCalledWith({ target: 'notifications' });
    expect(cb.osOpen).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });

  it.each([
    'https://example.com/Users/me/foo.dmg',
    'http://example.com/foo.dmg',
  ])('OS-OPEN: external URL %s is NOT OS-opened (keeps browser behavior)', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.osOpen).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  it('OS-OPEN: relative workspace path (data/…) previews in-app, never OS-opens', () => {
    const e = mkEvent(mkAnchor('data/artifacts/report.pdf'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.osOpen).not.toHaveBeenCalled();
    expect(cb.openArtifact).toHaveBeenCalledWith('artifacts/report.pdf');
    expect(e.defaultPrevented).toBe(true);
  });

  // ---------------------------------------------------------------------------
  // The reported bug: `lucidos data write` lands an artifact and prints
  // `[name](artifacts/<path>)` for the agent to paste. The artifacts cache is
  // SSE-refreshed and does not have the path yet, so linkifyPaths leaves a raw
  // relative href; with no data-path branch here the browser navigated to
  // /<slug>/artifacts/... , the SPA fallback served the app shell, and the whole
  // workspace reloaded.
  // ---------------------------------------------------------------------------

  it.each([
    ['artifacts/pr-review/pr-1582/index.html', 'artifacts/pr-review/pr-1582/index.html'],
    ['data/artifacts/report.html', 'artifacts/report.html'],
    ['/artifacts/report.html', 'artifacts/report.html'],
    ['/data/artifacts/report.html', 'artifacts/report.html'],
    ['knowhow/myapp/notes.md', 'knowhow/myapp/notes.md'],
    ['triggers/daily/run.md', 'triggers/daily/run.md'],
    ['system-knowhow/js-sdk.md', 'system-knowhow/js-sdk.md'],
    ['artifacts/report.html?v=2', 'artifacts/report.html'],
    ['artifacts/report.html#top', 'artifacts/report.html'],
  ])('DATA PATH %s → openArtifact(%s)', (href, expected) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openArtifact).toHaveBeenCalledWith(expected);
    expect(cb.osOpen).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });

  it.each([
    '/artifacts/report.pdf',
    '/knowhow/x.md',
    '/triggers/daily/run.md',
    '/system-knowhow/js-sdk.md',
  ])('DATA PATH absolute %s is never handed to the OS opener', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.osOpen).not.toHaveBeenCalled();
    expect(cb.openArtifact).toHaveBeenCalledOnce();
  });

  // ---------------------------------------------------------------------------
  // Terminal guard. The branches above are a whitelist, and a whitelist is open
  // at the bottom. An unclaimed relative href used to escape to the browser and
  // reload the workspace; an unclaimed SCHEME used to escape and do nothing at
  // all. Neither escapes now.
  // ---------------------------------------------------------------------------

  it.each([
    'README',
    'some/unknown/path.md',
    'unknown-panel',
    'artifacts',          // a bare sub-tree is a directory, not a file
    'config/apis.json',   // a real data/ sub-tree, but not one served for preview
    '/data',
  ])('CLOSED: unclaimed relative href %s is swallowed with a toast', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(e.defaultPrevented).toBe(true);
    expect(cb.toast).toHaveBeenCalledOnce();
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(cb.navigate).not.toHaveBeenCalled();
    expect(cb.openArtifact).not.toHaveBeenCalled();
    expect(cb.osOpen).not.toHaveBeenCalled();
  });

  it('CLOSED: the toast names the offending href', () => {
    const e = mkEvent(mkAnchor('some/unknown/path.md'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.toast).toHaveBeenCalledWith(expect.stringContaining('some/unknown/path.md'));
  });

  it('CLOSED: an empty href is swallowed and reported without an empty name', () => {
    // `[click here]()` renders `<a href="">`, which resolves to the current URL
    // and reloads exactly like any other unclaimed relative href. It has to be
    // swallowed, but it cannot be named.
    const e = mkEvent(mkAnchor(''));
    runHandleLinkClick(e, APPS, cb);
    expect(e.defaultPrevented).toBe(true);
    expect(cb.toast).toHaveBeenCalledWith('This link has no destination');
  });

  it.each([
    '#section',            // in-page markdown anchor: navigates nothing
    'https://example.com', // real external link
    'http://example.com/x',
    'mailto:a@example.com',
    'tel:+4712345678',
    'sms:+4712345678',
  ])('CLOSED: %s passes through untouched', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(e.defaultPrevented).toBe(false);
    expect(cb.toast).not.toHaveBeenCalled();
  });

  it.each([
    'note:abc',            // the shape of the reported bug, before trigger: was claimed
    'change:4f2c1a90',
    'vscode://file/tmp/x',
    'thread:not-a-uuid',   // malformed, so the markdown rewriter declined it
  ])('CLOSED: unopenable scheme %s is swallowed, never left silent', (href) => {
    // The reported bug: `trigger:<uuid>` carried a scheme, the guard exempted
    // every scheme, and the browser had no handler. The click did nothing and
    // said nothing, which reads as a dead app rather than a dead link.
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(e.defaultPrevented).toBe(true);
    expect(cb.toast).toHaveBeenCalledWith(expect.stringContaining('scheme'));
    expect(cb.toast).toHaveBeenCalledWith(expect.stringContaining(href));
  });

  it('CLOSED: an unopenable scheme reads differently from an unresolved path', () => {
    // Different causes, different fixes, so the two must not share wording.
    runHandleLinkClick(mkEvent(mkAnchor('note:abc')), APPS, cb);
    runHandleLinkClick(mkEvent(mkAnchor('some/unknown/path.md')), APPS, cb);
    const [scheme, relative] = cb.toast.mock.calls.map((c: unknown[]) => c[0] as string);
    expect(scheme).not.toBe(relative);
    expect(relative).toContain('points nowhere in this workspace');
  });

  it('the app OWN schemes never reach the guard', () => {
    // Each is claimed by its extractor first, so closing the guard cannot make
    // one of them toast.
    for (const href of ['app:habit-tracker', 'trigger:abc-123', '/Applications/X.app']) {
      cb.toast.mockClear();
      runHandleLinkClick(mkEvent(mkAnchor(href)), APPS, cb);
      expect(cb.toast, `${href} must not reach the terminal guard`).not.toHaveBeenCalled();
    }
  });
});

describe('chat link click — handler structure pin', () => {
  // Catches a future edit that quietly changes the branch structure of the
  // real handleLinkClick in ChatExchange.tsx, which would let the in-test
  // mirror (runHandleLinkClick above) drift and give us false confidence.
  it('handleLinkClick has the six branches in the documented order, each terminated by return;', () => {
    // The six branches must appear in this order: image → artifact → app →
    // trigger → nav → anchor-fallback. Each branch must close with `return;` before
    // the next `.closest(...)` opens — otherwise a refactor that swaps two
    // if-bodies (e.g. matches the targets in one order but acts on them
    // in another) would slip past a simpler text-order pin. The lazy
    // quantifier guarantees the first `return;` after each selector is the
    // boundary, so reordering forces a regex break.
    const m = chatExchangeSource.match(/function handleLinkClick[\s\S]*?\n  \}\n/);
    expect(m, 'handleLinkClick not found in ChatExchange.tsx').not.toBeNull();
    const body = m![0];
    const sequence =
      /closest\('\.image-thumbnail'\)[\s\S]+?return;[\s\S]+?closest\('\.artifact-link'\)[\s\S]+?return;[\s\S]+?closest\('\.app-link'\)[\s\S]+?return;[\s\S]+?closest\('\.trigger-link'\)[\s\S]+?return;[\s\S]+?closest\('\.nav-link'\)[\s\S]+?return;[\s\S]+?closest\('a'\)/;
    expect(body).toMatch(sequence);
  });

  it('handleLinkClick uses all six href extractors in the fallback branch', () => {
    expect(chatExchangeSource).toMatch(/import.*extractAppIdFromHref.*extractNavTargetFromHref.*extractLocalFileTarget.*extractBareAppRef.*extractDataPathTarget.*extractTriggerIdFromHref.*from.*linkifyPaths/);
    expect(chatExchangeSource).toMatch(/extractAppIdFromHref\(rawHref\)/);
    expect(chatExchangeSource).toMatch(/extractTriggerIdFromHref\(rawHref\)/);
    expect(chatExchangeSource).toMatch(/extractNavTargetFromHref\(rawHref\)/);
    expect(chatExchangeSource).toMatch(/extractBareAppRef\(rawHref\)/);
    expect(chatExchangeSource).toMatch(/extractDataPathTarget\(rawHref\)/);
    expect(chatExchangeSource).toMatch(/extractLocalFileTarget\(rawHref\)/);
  });

  it('fallback branch calls openApp, handleNavigationRequest, openFilePreview and openLocalFile with preventDefault', () => {
    const m = chatExchangeSource.match(/closest\('a'\)[\s\S]*?\n  \}\n/);
    expect(m).not.toBeNull();
    const body = m![0];
    expect(body).toContain('openApp(app)');
    expect(body).toContain('navigateToTrigger(triggerId)');
    expect(body).toContain('handleNavigationRequest({ target: navName })');
    expect(body).toContain('openFilePreview(dataPath)');
    expect(body).toContain('openLocalFile(localFile)');
    expect(body).toContain('e.preventDefault()');
  });

  it('the fallback extractors run in order: app, trigger, nav, bare-app-ref, data-path, OS-open', () => {
    // extractLocalFileTarget must appear after the app/nav extractors, or an
    // absolute /apps/… or /notifications href could be handed to the OS instead
    // of routed in-app. extractBareAppRef must run AFTER nav so a reserved panel
    // name (`notifications`) keeps routing to its panel, and BEFORE the OS-open
    // so a bare app-id href never falls through to the disk opener.
    // extractDataPathTarget sits between them: after bare-app-ref (which only
    // ever claims single-segment hrefs, so they cannot collide) and before
    // OS-open, so an absolute /artifacts/… is read as a workspace file rather
    // than a disk path.
    // The trigger extractor claims `trigger:` and nothing else, so its slot is
    // for narrative order rather than for resolving a collision.
    const appIdx = chatExchangeSource.indexOf('extractAppIdFromHref(rawHref)');
    const trigIdx = chatExchangeSource.indexOf('extractTriggerIdFromHref(rawHref)');
    const navIdx = chatExchangeSource.indexOf('extractNavTargetFromHref(rawHref)');
    const bareIdx = chatExchangeSource.indexOf('extractBareAppRef(rawHref)');
    const dataIdx = chatExchangeSource.indexOf('extractDataPathTarget(rawHref)');
    const fileIdx = chatExchangeSource.indexOf('extractLocalFileTarget(rawHref)');
    expect(appIdx).toBeGreaterThanOrEqual(0);
    expect(trigIdx).toBeGreaterThan(appIdx);
    expect(navIdx).toBeGreaterThan(trigIdx);
    expect(bareIdx).toBeGreaterThan(navIdx);
    expect(dataIdx).toBeGreaterThan(bareIdx);
    expect(fileIdx).toBeGreaterThan(dataIdx);
  });

  it('the terminal guard is LAST and swallows every scheme-less href', () => {
    // The whole point of the guard is that nothing follows it: it is the
    // bottom of the whitelist. A new extractor added AFTER it would be dead
    // code, and, worse, would read as covering a shape the guard already ate.
    const m = chatExchangeSource.match(/function handleLinkClick[\s\S]*?\n  \}\n/);
    expect(m, 'handleLinkClick not found in ChatExchange.tsx').not.toBeNull();
    const body = m![0];
    const guard = body.indexOf('showToast(');
    expect(guard, 'terminal guard toast not found').toBeGreaterThan(0);
    expect(body.indexOf('extractLocalFileTarget(rawHref)')).toBeLessThan(guard);
    expect(body.slice(guard)).not.toMatch(/extract[A-Za-z]+\(rawHref\)/);
    // It must gate on BOTH exemptions: a scheme the browser can act on, and a
    // pure fragment. `browserHandlesHref` is the shared helper, built on the
    // shared `hasUrlScheme`, so no router carries its own idea of a scheme.
    expect(body).toContain('!browserHandlesHref(rawHref)');
    expect(body).toContain("!rawHref.startsWith('#')");
  });

  it('no router re-implements the URL-scheme test inline', () => {
    // Four copies of `/^[a-z][a-z0-9+.-]*:/i` existed across the chat handler,
    // the two extractors, and the preview bridge. They are one exported
    // `hasUrlScheme` now; an inline copy drifts the routers apart silently.
    const sources = [
      ['ChatExchange.tsx', chatExchangeSource],
      ['linkifyPaths.ts', readFileSync(resolve(here, '../../../utils/linkifyPaths.ts'), 'utf-8')],
      ['previewIframeLinks.ts', readFileSync(resolve(here, '../../files/previewIframeLinks.ts'), 'utf-8')],
    ] as const;
    for (const [name, src] of sources) {
      const inline = src.match(/\/\^\[a-z\]\[a-z0-9\+\.-\]\*:\/i/g) ?? [];
      // linkifyPaths.ts holds the ONE definition inside `hasUrlScheme`.
      const allowed = name === 'linkifyPaths.ts' ? 1 : 0;
      expect(inline.length, `${name} must not inline the scheme regex`).toBe(allowed);
    }
  });
});
