// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import { linkifyPaths, extractAppTargetFromHref, extractNavTargetFromHref, extractLocalFileTarget, extractBareAppRef, extractDataPathTarget, extractWorkspaceDiskDataPath, extractTriggerIdFromHref, extractSettingsTargetFromHref, extractRepoFileTargetFromHref, hasUrlScheme, browserHandlesHref, type RepoFileHrefTarget } from '../../../utils/linkifyPaths';
import { renderMarkdown } from '../../../utils/renderMarkdown';
import type { App } from '../../../store/types';

const here: string = dirname(fileURLToPath(import.meta.url));
const routerSource = readFileSync(resolve(here, '../../shared/markdownLinkClick.ts'), 'utf-8');

const WORKSPACE = '/Users/me/workspaces/dev';

const APPS: App[] = [
  { id: 'work-tracker', name: 'Lucidos Work', description: 'x', reveal: 'on-load', kind: 'app', reusable: false },
  { id: 'habit-tracker', name: 'Habit Tracker', description: 'y', reveal: 'on-load', kind: 'app', reusable: false },
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
      if (selector === 'a') return el;
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

type Callbacks = {
  openImage: (src: string, target: any) => void;
  openArtifact: (path: string) => void;
  openApp: (app: App) => void;
  openAppById: (id: string, fragment?: string) => void;
  openTrigger: (id: string) => void;
  openRepoFile: (target: RepoFileHrefTarget) => void;
  navigate: (req: { target: string; settings_view?: string; fragment?: string }) => void;
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
    if (id) cb.openAppById(id, (app as any).dataset.appFragment);
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
    if (target) {
      cb.navigate({
        target,
        settings_view: (nav as any).dataset.settingsView,
        fragment: (nav as any).dataset.settingsFragment,
      });
    }
    return;
  }
  const anchor = t.closest('a');
  if (anchor) {
    const href = anchor.getAttribute('href') || '';
    const appRef = extractAppTargetFromHref(href);
    if (appRef) {
      e.preventDefault();
      cb.openAppById(appRef.appId, appRef.fragment ?? undefined);
      return;
    }
    const triggerId = extractTriggerIdFromHref(href);
    if (triggerId) {
      e.preventDefault();
      cb.openTrigger(triggerId);
      return;
    }
    const repoFile = extractRepoFileTargetFromHref(href);
    if (repoFile) {
      e.preventDefault();
      cb.openRepoFile(repoFile);
      return;
    }
    const settings = extractSettingsTargetFromHref(href);
    if (settings) {
      e.preventDefault();
      cb.navigate({ target: 'settings', settings_view: settings.view, fragment: settings.fragment ?? undefined });
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
    const dataPath = extractDataPathTarget(href) ?? extractWorkspaceDiskDataPath(href, WORKSPACE);
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
    openAppById: ReturnType<typeof vi.fn>;
    openTrigger: ReturnType<typeof vi.fn>;
    openRepoFile: ReturnType<typeof vi.fn>;
    navigate: ReturnType<typeof vi.fn>;
    osOpen: ReturnType<typeof vi.fn>;
    toast: ReturnType<typeof vi.fn>;
  };

  beforeEach(() => {
    cb = {
      openImage: vi.fn() as Callbacks['openImage'] & ReturnType<typeof vi.fn>,
      openArtifact: vi.fn() as Callbacks['openArtifact'] & ReturnType<typeof vi.fn>,
      openApp: vi.fn() as Callbacks['openApp'] & ReturnType<typeof vi.fn>,
      openAppById: vi.fn() as Callbacks['openAppById'] & ReturnType<typeof vi.fn>,
      openTrigger: vi.fn() as Callbacks['openTrigger'] & ReturnType<typeof vi.fn>,
      openRepoFile: vi.fn() as Callbacks['openRepoFile'] & ReturnType<typeof vi.fn>,
      navigate: vi.fn() as Callbacks['navigate'] & ReturnType<typeof vi.fn>,
      osOpen: vi.fn() as Callbacks['osOpen'] & ReturnType<typeof vi.fn>,
      toast: vi.fn() as Callbacks['toast'] & ReturnType<typeof vi.fn>,
    };
  });

  it('PRIMARY: pre-rewritten <a class="app-link"> click → openAppById', () => {
    const a = mkAnchor('#', 'app-link', { appId: 'work-tracker' });
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openAppById).toHaveBeenCalledWith('work-tracker', undefined);
    expect(e.defaultPrevented).toBe(true);
  });

  it('PRIMARY: .app-link with an id not in the cached apps list still routes to openAppById', () => {
    const a = mkAnchor('#', 'app-link', { appId: 'unknown-app' });
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openAppById).toHaveBeenCalledWith('unknown-app', undefined);
    expect(cb.toast).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });

  it('FALLBACK: plain <a href="apps/<id>/index.html"> click → openAppById', () => {
    const a = mkAnchor('apps/work-tracker/index.html');
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openAppById).toHaveBeenCalledWith('work-tracker', undefined);
    expect(e.defaultPrevented).toBe(true);
  });

  it.each([
    '/apps/work-tracker/index.html',
    'data/apps/work-tracker/index.html',
    '/data/apps/work-tracker/index.html',
    'apps/work-tracker',
    'apps/work-tracker/',
    'apps/work-tracker/index.html?v=2',
    'app:work-tracker',
    'app:work-tracker/',
    'app:work-tracker?refresh=1',
  ])('FALLBACK entry-point: %s → openAppById with no target', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openAppById).toHaveBeenCalledWith('work-tracker', undefined);
    expect(e.defaultPrevented).toBe(true);
  });

  it.each([
    ['app:work-tracker#pr-1645', 'pr-1645'],
    ['apps/work-tracker/index.html#section', 'section'],
    ['apps/work-tracker#section', 'section'],
    ['app:work-tracker?v=2#section', 'section'],
    ['app:work-tracker#', undefined],
  ])('FALLBACK entry-point: %s → openAppById at %s', (href, fragment) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openAppById).toHaveBeenCalledWith('work-tracker', fragment);
    expect(e.defaultPrevented).toBe(true);
  });

  it('PRIMARY: a pre-rewritten .app-link carries its data-app-fragment', () => {
    const a = mkAnchor('#', 'app-link', { appId: 'work-tracker', appFragment: 'pr-1645' });
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openAppById).toHaveBeenCalledWith('work-tracker', 'pr-1645');
  });

  it('REGRESSION: app:<id> not in the cached apps list routes to openAppById, never the dead-link toast', () => {
    const e = mkEvent(mkAnchor('app:pulse'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openAppById).toHaveBeenCalledWith('pulse', undefined);
    expect(cb.toast).not.toHaveBeenCalled();
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

  it('unknown app id (apps/<id>/index.html shape) → routes to openAppById, never falls through to file preview', () => {
    const e = mkEvent(mkAnchor('apps/no-such-app/index.html'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openAppById).toHaveBeenCalledWith('no-such-app', undefined);
    expect(cb.openArtifact).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });

  it('does NOT intercept external https URLs that happen to contain apps/', () => {
    const e = mkEvent(mkAnchor('https://example.com/apps/work-tracker/index.html'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });


  it.each([
    'work-tracker',
    '/work-tracker',
    'work-tracker/',
    'work-tracker?v=2',
    'work-tracker#top',
  ])('BARE app-id href %s → openApp', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openApp).toHaveBeenCalledWith(APPS[0]);
    expect(e.defaultPrevented).toBe(true);
  });

  it('BARE app-NAME href, percent-encoded (Habit%20Tracker) → openApp', () => {
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
    const md = 'Open it in [Lucidos Work](apps/work-tracker/index.html).';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="app-link"');
    expect(html).toContain('data-app-id="work-tracker"');
    expect(html).toContain('>Lucidos Work</a>');
  });

  it('END-TO-END: render → linkify pipeline rewrites app:<id> custom scheme', () => {
    const md = 'Open the [Habit Tracker](app:habit-tracker) and switch to the Backtest tab.';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="app-link"');
    expect(html).toContain('data-app-id="habit-tracker"');
    expect(html).toContain('>Habit Tracker</a>');
    expect(html).not.toContain('href="app:');
  });



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
    const md = 'Here it is: [Nightly digest](trigger:3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63)';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="trigger-link"');
    expect(html).toContain('data-trigger-id="3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63"');
    expect(html).toContain('>Nightly digest</a>');
  });


  it('PRIMARY: a rewritten settings nav-link carries its view to the router', () => {
    const a = mkAnchor('#', 'nav-link', { navTarget: 'settings', settingsView: 'backup' });
    const e = mkEvent(a);
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).toHaveBeenCalledWith({ target: 'settings', settings_view: 'backup' });
    expect(e.defaultPrevented).toBe(true);
  });

  it('PRIMARY: a rewritten settings row link carries its fragment to the router', () => {
    const a = mkAnchor('#', 'nav-link', { navTarget: 'settings', settingsView: 'providers', settingsFragment: 'xai' });
    runHandleLinkClick(mkEvent(a), APPS, cb);
    expect(cb.navigate).toHaveBeenCalledWith({ target: 'settings', settings_view: 'providers', fragment: 'xai' });
  });

  it('FALLBACK: plain <a href="settings:<view>#<row>"> click names the row', () => {
    runHandleLinkClick(mkEvent(mkAnchor('settings:providers#xai')), APPS, cb);
    expect(cb.navigate).toHaveBeenCalledWith({ target: 'settings', settings_view: 'providers', fragment: 'xai' });
  });

  it('FALLBACK: plain <a href="settings:<view>"> click → that Settings page, never the guard', () => {
    const e = mkEvent(mkAnchor('settings:backup'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.navigate).toHaveBeenCalledWith({ target: 'settings', settings_view: 'backup' });
    expect(cb.toast).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });

  it('END-TO-END: render → linkify yields the settings nav-link the click expects', () => {
    const md = 'Open [Settings → System → Backup](settings:backup) to see the details and retry.';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('class="nav-link"');
    expect(html).toContain('data-nav-target="settings"');
    expect(html).toContain('data-settings-view="backup"');
    expect(html).toContain('>Settings → System → Backup</a>');
  });

  it('the real router claims settings: before the panel names', () => {
    const settingsIdx = routerSource.indexOf('extractSettingsTargetFromHref(rawHref)');
    const navIdx = routerSource.indexOf('extractNavTargetFromHref(rawHref)');
    expect(settingsIdx).toBeGreaterThan(0);
    expect(navIdx).toBeGreaterThan(settingsIdx);
    expect(routerSource).toMatch(/settings_view: navTarget\.dataset\.settingsView/);
    expect(routerSource).toMatch(/fragment: navTarget\.dataset\.settingsFragment/);
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
    const md = 'Open it: [Notifications](data/notifications).';
    const html = linkifyPaths(renderMarkdown(md), [], APPS);
    expect(html).toContain('href="#"');
    expect(html).toContain('class="nav-link"');
    expect(html).toContain('data-nav-target="notifications"');
    expect(html).toContain('>Notifications</a>');
  });


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
    const e = mkEvent(mkAnchor('/apps/work-tracker/index.html'));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openAppById).toHaveBeenCalledWith('work-tracker', undefined);
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

  it.each([
    `${WORKSPACE}/data/artifacts/marketing/phone-100.png`,
    `file://${WORKSPACE}/data/artifacts/marketing/phone-100.png`,
  ])('DISK PATH: %s inside the workspace previews in-app, never OS-opens', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(cb.openArtifact).toHaveBeenCalledWith('artifacts/marketing/phone-100.png');
    expect(cb.osOpen).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(true);
  });


  it.each([
    ['artifacts/pr-review/pr-1582/index.html', 'artifacts/pr-review/pr-1582/index.html'],
    ['data/artifacts/report.html', 'artifacts/report.html'],
    ['/artifacts/report.html', 'artifacts/report.html'],
    ['/data/artifacts/report.html', 'artifacts/report.html'],
    ['knowhow/myapp/notes.md', 'knowhow/myapp/notes.md'],
    ['triggers/daily/run.md', 'triggers/daily/run.md'],
    ['system-knowhow/js-sdk.md', 'system-knowhow/js-sdk.md'],
    ['themes/harbour.json', 'themes/harbour.json'],
    ['config/apis.json', 'config/apis.json'],
    ['scripts/auth/login.py', 'scripts/auth/login.py'],
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


  it.each([
    'README',
    'some/unknown/path.md',
    'unknown-panel',
    'artifacts',
    'postgres/pg.conf',
    '/data',
  ])('CLOSED: unclaimed relative href %s is swallowed with a toast', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(e.defaultPrevented).toBe(true);
    expect(cb.toast).toHaveBeenCalledOnce();
    expect(cb.openApp).not.toHaveBeenCalled();
    expect(cb.openAppById).not.toHaveBeenCalled();
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
    const e = mkEvent(mkAnchor(''));
    runHandleLinkClick(e, APPS, cb);
    expect(e.defaultPrevented).toBe(true);
    expect(cb.toast).toHaveBeenCalledWith('This link has no destination');
  });

  it.each([
    '#section',
    'https://example.com',
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
    'note:abc',
    'change:4f2c1a90',
    'vscode://file/tmp/x',
    'thread:not-a-uuid',
  ])('CLOSED: unopenable scheme %s is swallowed, never left silent', (href) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(e.defaultPrevented).toBe(true);
    expect(cb.toast).toHaveBeenCalledWith(expect.stringContaining('scheme'));
    expect(cb.toast).toHaveBeenCalledWith(expect.stringContaining(href));
  });

  it('CLOSED: an unopenable scheme reads differently from an unresolved path', () => {
    runHandleLinkClick(mkEvent(mkAnchor('note:abc')), APPS, cb);
    runHandleLinkClick(mkEvent(mkAnchor('some/unknown/path.md')), APPS, cb);
    const [scheme, relative] = cb.toast.mock.calls.map((c: unknown[]) => c[0] as string);
    expect(scheme).not.toBe(relative);
    expect(relative).toContain('points nowhere in this workspace');
  });

  it('the app OWN schemes never reach the guard', () => {
    for (const href of ['app:habit-tracker', 'trigger:abc-123', 'repo:lucidos:file:README.md', '/Applications/X.app']) {
      cb.toast.mockClear();
      runHandleLinkClick(mkEvent(mkAnchor(href)), APPS, cb);
      expect(cb.toast, `${href} must not reach the terminal guard`).not.toHaveBeenCalled();
    }
  });
});

describe('chat link click — repository file links', () => {
  const openRepoFile = vi.fn();
  const cb = {
    openImage: vi.fn(), openArtifact: vi.fn(), openApp: vi.fn(), openAppById: vi.fn(),
    openTrigger: vi.fn(), openRepoFile, navigate: vi.fn(), osOpen: vi.fn(), toast: vi.fn(),
  };
  beforeEach(() => { for (const f of Object.values(cb)) f.mockClear(); });

  it.each([
    ['repo:lucidos:file:crates/lucidos-app/src/main.tsx', 'lucidos', undefined, 'crates/lucidos-app/src/main.tsx'],
    ['repo:6f1c2a90-0000-5000-8000-000000000001:file:README.md', '6f1c2a90-0000-5000-8000-000000000001', undefined, 'README.md'],
    ['repo:lucidos:file#v1.2.0:docs/adr/index.md', 'lucidos', 'v1.2.0', 'docs/adr/index.md'],
  ])('%s → repository file preview', (href, repoId, ref, path) => {
    const e = mkEvent(mkAnchor(href));
    runHandleLinkClick(e, APPS, cb);
    expect(e.defaultPrevented).toBe(true);
    expect(openRepoFile).toHaveBeenCalledWith({ locator: { repoId, mode: 'file', ref, path } });
    expect(cb.osOpen).not.toHaveBeenCalled();
    expect(cb.toast).not.toHaveBeenCalled();
  });

  it('END-TO-END: a rendered repo link keeps its href for the router', () => {
    const html = linkifyPaths(renderMarkdown('[main](repo:lucidos:file:src/main.rs)'), [], APPS);
    expect(html).toContain('href="repo:lucidos:file:src/main.rs"');
  });
});

describe('chat link click — handler structure pin', () => {
  it('handleMarkdownLinkClick has the six branches in the documented order, each terminated by return;', () => {
    const m = routerSource.match(/function handleMarkdownLinkClick[\s\S]*?\n\}\n/);
    expect(m, 'handleMarkdownLinkClick not found in markdownLinkClick.ts').not.toBeNull();
    const body = m![0];
    const sequence =
      /closest\('\.image-thumbnail'\)[\s\S]+?return;[\s\S]+?closest\('\.artifact-link'\)[\s\S]+?return;[\s\S]+?closest\('\.app-link'\)[\s\S]+?return;[\s\S]+?closest\('\.trigger-link'\)[\s\S]+?return;[\s\S]+?closest\('\.nav-link'\)[\s\S]+?return;[\s\S]+?closest\('a'\)/;
    expect(body).toMatch(sequence);
  });

  it('handleMarkdownLinkClick uses all eight href extractors in the fallback branch', () => {
    expect(routerSource).toMatch(/import[\s\S]*?extractAppTargetFromHref[\s\S]*?extractNavTargetFromHref[\s\S]*?extractLocalFileTarget[\s\S]*?extractBareAppRef[\s\S]*?extractDataPathTarget[\s\S]*?extractTriggerIdFromHref[\s\S]*?extractRepoFileTargetFromHref[\s\S]*?from '..\/..\/utils\/linkifyPaths'/);
    expect(routerSource).toMatch(/extractAppTargetFromHref\(rawHref\)/);
    expect(routerSource).toMatch(/extractTriggerIdFromHref\(rawHref\)/);
    expect(routerSource).toMatch(/extractRepoFileTargetFromHref\(rawHref\)/);
    expect(routerSource).toMatch(/extractNavTargetFromHref\(rawHref\)/);
    expect(routerSource).toMatch(/extractBareAppRef\(rawHref\)/);
    expect(routerSource).toMatch(/extractDataPathTarget\(rawHref\)/);
    expect(routerSource).toMatch(/extractWorkspaceDiskDataPath\(rawHref, workspacePath\.value\)/);
    expect(routerSource).toMatch(/extractLocalFileTarget\(rawHref\)/);
  });

  it('fallback branch calls openAppById, openApp, openRepoFileLink, handleNavigationRequest, the file preview modal and openLocalFileOnConfirm with preventDefault', () => {
    const m = routerSource.match(/closest\('a'\)[\s\S]*?\n\}\n/);
    expect(m).not.toBeNull();
    const body = m![0];
    expect(body).toContain(
      'openAppById(appTargetRef.appId, source, appTargetRef.fragment ?? undefined)',
    );
    expect(body).toContain('openApp(app)');
    expect(body).toContain('navigateToTrigger(triggerId, source)');
    expect(body).toMatch(/openRepoFileLink\([\s\S]*?openFilePreviewModal,?\s*\)/);
    expect(body).toContain('handleNavigationRequest({ target: navName })');
    expect(body).toContain('previewDataFile(dataPath, ');
    expect(body).toContain('openLocalFileOnConfirm(localFile, source)');
    expect(body).toContain('e.preventDefault()');
  });

  it('the app extractor arm never gates on the cached apps list before opening', () => {
    const m = routerSource.match(/const appTargetRef = extractAppTargetFromHref\(rawHref\);[\s\S]*?\n    const triggerId/);
    expect(m, 'extractAppTargetFromHref arm not found').not.toBeNull();
    expect(m![0]).not.toContain('apps.find');
  });

  it('the fallback extractors run in order: app, trigger, repo, nav, bare-app-ref, data-path, OS-open', () => {
    const appIdx = routerSource.indexOf('extractAppTargetFromHref(rawHref)');
    const trigIdx = routerSource.indexOf('extractTriggerIdFromHref(rawHref)');
    const repoIdx = routerSource.indexOf('extractRepoFileTargetFromHref(rawHref)');
    const navIdx = routerSource.indexOf('extractNavTargetFromHref(rawHref)');
    const bareIdx = routerSource.indexOf('extractBareAppRef(rawHref)');
    const dataIdx = routerSource.indexOf('extractDataPathTarget(rawHref)');
    const diskIdx = routerSource.indexOf('extractWorkspaceDiskDataPath(rawHref');
    const fileIdx = routerSource.indexOf('extractLocalFileTarget(rawHref)');
    expect(appIdx).toBeGreaterThanOrEqual(0);
    expect(trigIdx).toBeGreaterThan(appIdx);
    expect(repoIdx).toBeGreaterThan(trigIdx);
    expect(navIdx).toBeGreaterThan(repoIdx);
    expect(bareIdx).toBeGreaterThan(navIdx);
    expect(dataIdx).toBeGreaterThan(bareIdx);
    expect(diskIdx).toBeGreaterThan(dataIdx);
    expect(fileIdx).toBeGreaterThan(diskIdx);
  });

  it('the terminal guard is LAST and swallows every scheme-less href', () => {
    const m = routerSource.match(/function handleMarkdownLinkClick[\s\S]*?\n\}\n/);
    expect(m, 'handleMarkdownLinkClick not found in markdownLinkClick.ts').not.toBeNull();
    const body = m![0];
    const guard = body.indexOf('showToast(');
    expect(guard, 'terminal guard toast not found').toBeGreaterThan(0);
    expect(body.indexOf('extractLocalFileTarget(rawHref)')).toBeLessThan(guard);
    expect(body.slice(guard)).not.toMatch(/extract[A-Za-z]+\(rawHref\)/);
    expect(body).toContain('!browserHandlesHref(rawHref)');
    expect(body).toContain("!rawHref.startsWith('#')");
  });

  it('no router re-implements the URL-scheme test inline', () => {
    const sources = [
      ['markdownLinkClick.ts', routerSource],
      ['linkifyPaths.ts', readFileSync(resolve(here, '../../../utils/linkifyPaths.ts'), 'utf-8')],
      ['previewIframeLinks.ts', readFileSync(resolve(here, '../../files/previewIframeLinks.ts'), 'utf-8')],
    ] as const;
    for (const [name, src] of sources) {
      const inline = src.match(/\/\^\[a-z\]\[a-z0-9\+\.-\]\*:\/i/g) ?? [];
      const allowed = name === 'linkifyPaths.ts' ? 1 : 0;
      expect(inline.length, `${name} must not inline the scheme regex`).toBe(allowed);
    }
  });
});
