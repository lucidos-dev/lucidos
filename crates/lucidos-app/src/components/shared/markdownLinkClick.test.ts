// @vitest-environment jsdom
// The sanitizer and the click both need a real DOM.

// Clicks real rendered agent markdown through the shared router. The reported
// bug: a notification body rendered `[changelog](artifacts/…)` as an
// `a.artifact-link`, but its own click handler only knew app and trigger
// links, so the tap did nothing. Every surface now shares this router.
import { describe, it, expect, beforeEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const mocks = vi.hoisted(() => ({
  openFilePreview: vi.fn(),
  openLocalFile: vi.fn(),
  openApp: vi.fn(),
  openAppById: vi.fn(async () => {}),
  navigateToTrigger: vi.fn(async () => {}),
  handleNavigationRequest: vi.fn(),
  showToast: vi.fn(),
}));

vi.mock('../../store/actions/artifacts', () => ({
  openFilePreview: mocks.openFilePreview,
  openLocalFile: mocks.openLocalFile,
}));
vi.mock('../../store/actions/apps', () => ({
  openApp: mocks.openApp,
  openAppById: mocks.openAppById,
}));
vi.mock('../../store/actions/triggers', () => ({ navigateToTrigger: mocks.navigateToTrigger }));
vi.mock('../../store/actions/navigation-request', () => ({
  handleNavigationRequest: mocks.handleNavigationRequest,
}));
vi.mock('../../store/store', async () => {
  const actual = await vi.importActual<typeof import('../../store/store')>('../../store/store');
  return { ...actual, showToast: mocks.showToast };
});

import { handleMarkdownLinkClick } from './markdownLinkClick';
import { linkifyPaths } from '../../utils/linkifyPaths';
import { renderMarkdown } from '../../utils/renderMarkdown';
import type { App } from '../../store/types';

const APPS: App[] = [{ id: 'pr-understanding', name: 'PR Understanding', description: '' }];

/** Render markdown the way the notification detail does (no known artifact
 *  paths), click its first anchor, and report whether the default was stopped. */
function clickFirstLink(markdown: string, source?: string): boolean {
  const host = document.createElement('div');
  host.innerHTML = linkifyPaths(renderMarkdown(markdown), [], APPS);
  let prevented = false;
  host.addEventListener('click', (e) => {
    handleMarkdownLinkClick(e, APPS, source);
    prevented = e.defaultPrevented;
    // jsdom cannot navigate; stop a link the router left to the browser.
    e.preventDefault();
  });
  document.body.appendChild(host);
  const anchor = host.querySelector('a');
  expect(anchor, `no anchor rendered for ${markdown}`).not.toBeNull();
  anchor!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  host.remove();
  return prevented;
}

describe('handleMarkdownLinkClick', () => {
  beforeEach(() => {
    for (const m of Object.values(mocks)) m.mockClear();
  });

  it('opens an artifact link in the file preview (the reported bug)', () => {
    const prevented = clickFirstLink(
      'Changelog: [artifacts/releases/changelog-v0.40.1.md](artifacts/releases/changelog-v0.40.1.md)',
      'a notification',
    );
    expect(prevented).toBe(true);
    expect(mocks.openFilePreview).toHaveBeenCalledWith('artifacts/releases/changelog-v0.40.1.md');
  });

  it('hands a file:// link to the OS opener', () => {
    const prevented = clickFirstLink('[DMG](file:///Users/me/Lucidos.dmg)');
    expect(prevented).toBe(true);
    expect(mocks.openLocalFile).toHaveBeenCalledWith('file:///Users/me/Lucidos.dmg');
  });

  it('hands a bare file:// URL in prose to the OS opener, even under an email-shaped home folder', () => {
    const prevented = clickFirstLink('DMG: file:///Users/me.x@example.com/p/Lucidos.dmg');
    expect(prevented).toBe(true);
    expect(mocks.openLocalFile).toHaveBeenCalledWith('file:///Users/me.x@example.com/p/Lucidos.dmg');
  });

  it('routes a trigger link with the surface named as its source', () => {
    clickFirstLink('[Nightly digest](trigger:3f9b21c4-0a7e)', 'a notification');
    expect(mocks.navigateToTrigger).toHaveBeenCalledWith('3f9b21c4-0a7e', 'a notification');
  });

  it('hands an app link its fragment and source', () => {
    clickFirstLink('[Some report](app:pr-understanding#pr-1645)', 'a notification');
    expect(mocks.openAppById).toHaveBeenCalledWith('pr-understanding', 'a notification', 'pr-1645');
  });

  it('opens a panel link', () => {
    clickFirstLink('[Triggers](triggers)');
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith({ target: 'triggers' });
  });

  it('toasts an unresolvable relative link instead of reloading the workspace', () => {
    const prevented = clickFirstLink('[gone](some/unknown/path)');
    expect(prevented).toBe(true);
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining('points nowhere in this workspace'),
      'error',
      expect.anything(),
    );
  });

  it('leaves an https link to the browser', () => {
    const prevented = clickFirstLink('[site](https://example.com)');
    expect(prevented).toBe(false);
    expect(mocks.showToast).not.toHaveBeenCalled();
  });
});

describe('every surface rendering agent markdown uses the shared router', () => {
  const here: string = dirname(fileURLToPath(import.meta.url));
  it.each([
    ['../notifications/NotificationDetailInline.tsx', "handleMarkdownLinkClick(e, apps, 'a notification')"],
    ['../chat/ChatExchange.tsx', 'handleMarkdownLinkClick(e, apps)'],
  ])('%s', (path, call) => {
    expect(readFileSync(resolve(here, path), 'utf-8')).toContain(call);
  });
});
