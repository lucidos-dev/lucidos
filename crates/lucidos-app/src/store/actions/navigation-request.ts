import { navigateToNewChat, navigateToUrl } from './navigationDirect';
import { switchMenuItem, openSettingsSubview, setActiveMenu } from './menu';
import { submitInteractiveRequest } from '../interactiveDeliveryService';
import { openAppById, exitAppFullscreen } from './apps';
import { openFilePreview } from './artifacts';
import { resolveFileTarget } from './fileTarget';

import { navigateToTrigger } from './triggers';
import { focusThread, focusThreadOrBootstrap } from './threads';
import { isForThisDevice, type DeviceScopeActor } from './device-scope';
import { formatThreadLabel } from './thread-label';
import { pushNavState } from './navigation';
import { holdFocusedPaneWhileTyping, releaseTypingHold, revealContentPane } from './pane';

import { openEncodedRepoFilePreview } from './repositories';
import { openRepoFileLink } from './repoFileLink';

import { revealDashboard } from './dashboardNav';
import { fileFindScope, requestFind } from './find-request';

import { showToast, dismissToast, repositories, focusedThreadId, pluginScrollTarget, setPluginsInstalledOnly, parseRepoPath, normalizeLineRange, selectedLines, lineScrollTarget, filePreviewSource, SETTINGS_SUBVIEW_ITEMS, settingsSubviewLabel, aliasRetiredSettingsSubview, settingsLinkView, settingsLinkedFragment } from '../store';
import type { SettingsNavKey } from '../store';
import type { MenuItem } from '../types';

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function isUnknownRepositoryId(repoId: string): boolean {
  const list = repositories.value;
  if (list.status === 'loaded') return !list.data.some((r) => r.id === repoId);
  return !UUID_RE.test(repoId);
}

let settingsViewKeys: ReadonlySet<string> | null = null;
function isRenderableSettingsView(view: string): boolean {
  if (settingsViewKeys === null) {
    settingsViewKeys = new Set<string>(SETTINGS_SUBVIEW_ITEMS.map((i) => i.key));
  }
  return settingsViewKeys.has(view);
}

export function describeNavTarget(nav: {
  target: string;
  app_id?: string;
  fragment?: string;
  file_path?: string;
  purpose?: string;
  line?: number;
  line_end?: number;
  settings_view?: string;
}): string {
  switch (nav.target) {
    case 'app':
    case 'app-ui':
      if (!nav.app_id) return 'an app';
      return `app "${nav.app_id}${nav.fragment ? `#${nav.fragment}` : ''}"`;
    case 'thread':
      return 'a thread';
    case 'file': {
      if (!nav.file_path) return 'a file';
      const repo = parseRepoPath(nav.file_path);
      const range = normalizeLineRange(nav.line, nav.line_end);
      const at = range ? `:${range.start}` : '';
      return `file "${repo?.path ?? nav.file_path}${at}"`;
    }
    case 'trigger':
      return 'a trigger';
    case 'url':
      return nav.purpose === 'oauth' ? 'a sign-in page (expires shortly)' : 'a link';
    case 'new-app':
      return 'the new-app form';
    case 'new-trigger':
      return 'the new-trigger form';
    case 'new-chat':
      return 'a new chat';
    case 'settings': {
      if (!nav.settings_view) return 'settings';
      const view = aliasRetiredSettingsSubview(nav.settings_view);
      const label = settingsSubviewLabel(view as SettingsNavKey);
      return `${label ?? nav.settings_view} settings`;
    }
    case 'app-store':
      return 'the Plugins panel';
    case 'plugins':
      return 'your installed plugins';
    case 'thread-queue':
      return 'the Thread Queue';
    case 'dashboard':
      return 'the dashboard';
    case 'files':
    case 'apps':
    case 'triggers':
    case 'changes':
    case 'notifications':
      return `the ${nav.target} panel`;
    default:
      return 'a page';
  }
}

export function handleNavigationRequest(nav: {
  target: string;
  settings_view?: string;
  app_id?: string;
  fragment?: string;
  file_path?: string;
  line?: number;
  line_end?: number;
  url?: string;
  purpose?: string;
  id?: string;
  event_id?: string;
  prompt?: string;
  coding_agent?: string;
  folder?: string;
}, opts?: {
  source?: string;
  find?: string;
}): Promise<unknown> | void {
  const navAppId = nav.app_id;
  switch (nav.target) {
    case 'dashboard':
      revealDashboard();
      break;
    case 'files':
    case 'apps':
    case 'triggers':
    case 'changes':
    case 'notifications':
      switchMenuItem(nav.target as MenuItem);
      break;
    case 'thread-queue':
      openSettingsSubview('thread-queue');
      break;
    case 'app-store':
      setPluginsInstalledOnly(false);
      switchMenuItem('plugins');
      break;
    case 'plugins':
      setPluginsInstalledOnly(true);
      switchMenuItem('plugins');
      pluginScrollTarget.value = nav.id ?? null;
      break;
    case 'settings': {
      if (!nav.settings_view) {
        switchMenuItem('settings');
        break;
      }
      const named = aliasRetiredSettingsSubview(nav.settings_view);
      const view = nav.fragment ? settingsLinkView(named, nav.fragment) : named;
      if (isRenderableSettingsView(view)) {
        openSettingsSubview(view as SettingsNavKey);
        if (nav.fragment) settingsLinkedFragment.value = nav.fragment;
      } else {
        switchMenuItem('settings');
        showToast(`Unknown settings section: "${nav.settings_view}"`, 'error');
      }
      break;
    }
    case 'app':
    case 'app-ui':
      if (!navAppId) {
        showToast('Navigation target missing app_id', 'error');
        break;
      }
      return openAppById(navAppId, opts?.source, nav.fragment);
    case 'file': {
      if (!nav.file_path) {
        showToast('Navigation target missing file_path', 'error');
        break;
      }
      const { path: filePath, range } = resolveFileTarget(nav.file_path, nav.line, nav.line_end);
      const repoLocator = parseRepoPath(filePath);
      if (repoLocator && isUnknownRepositoryId(repoLocator.repoId)) {
        return openRepoFileLink({ locator: repoLocator, line: nav.line, lineEnd: nav.line_end }, opts?.source);
      }
      if (!openEncodedRepoFilePreview(filePath)) openFilePreview(filePath);
      if (range) {
        filePreviewSource.value = true;
        selectedLines.value = range;
        lineScrollTarget.value = { path: filePath, line: range.start };
        if (opts?.find) requestFind(fileFindScope(filePath), opts.find, range.start);
      }
      break;
    }
    case 'trigger':
      if (!nav.id) {
        showToast('Navigation target missing trigger id', 'error');
        break;
      }
      return navigateToTrigger(nav.id, opts?.source);
    case 'thread':
      if (!nav.id) {
        showToast('Navigation target missing thread id', 'error');
        break;
      }
      exitAppFullscreen();
      focusThreadOrBootstrap(nav.id, {
        targetEventId: nav.event_id ?? null,
        source: opts?.source,
      });
      break;
    case 'new-app':
      setActiveMenu('apps', { type: 'form', form: { type: 'new-app' } });
      pushNavState();
      revealContentPane();
      break;
    case 'new-trigger':
      setActiveMenu('triggers', { type: 'form', form: { type: 'trigger' } });
      pushNavState();
      revealContentPane();
      break;
    case 'new-chat': return navigateToNewChat(nav, opts?.source);
    case 'url': return navigateToUrl(nav, opts?.source);
    default:
      showToast(`Unknown navigation target: ${nav.target}`, 'error');
      break;
  }
}

export const NIL_THREAD_ID = '00000000-0000-0000-0000-000000000000';

export function routeThreadNavigation(
  nav: Parameters<typeof handleNavigationRequest>[0],
  actor: DeviceScopeActor | null | undefined,
  sourceThreadId: string,
  eventId?: string,
): void {
  if (!isForThisDevice(actor)) return;
  submitInteractiveRequest({
    kind: 'navigation', payload: { nav, sourceThreadId },
    preference: sourceThreadId === NIL_THREAD_ID ? { fromApp: true } : { threadId: sourceThreadId },
  }, eventId);
}

export function presentThreadNavigation(
  nav: Parameters<typeof handleNavigationRequest>[0], sourceThreadId: string,
): void {
  const fromApp = sourceThreadId === NIL_THREAD_ID;
  if (fromApp || sourceThreadId === focusedThreadId.value) {
    const source = fromApp ? 'an app' : formatThreadLabel(sourceThreadId);
    holdFocusedPaneWhileTyping();
    const pending = handleNavigationRequest(nav, { source });
    if (pending) void pending.then(releaseTypingHold, releaseTypingHold);
    else releaseTypingHold();
    return;
  }
  const label = formatThreadLabel(sourceThreadId);
  const key = `nav-offer-${sourceThreadId}`;
  showToast(`${label} wants to open ${describeNavTarget(nav)}`, 'info', {
    key,
    action: {
      label: 'Open',
      onClick: () => {
        dismissToast(key);
        focusThread(sourceThreadId);
        handleNavigationRequest(nav, { source: label });
      },
    },
  });
}
