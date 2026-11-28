import { activeMenuItem, panelOverlay, panelUrl, panelTitle, settingsSubview, settingsSubviewLabel, settingsSubviewShortLabel, triggers, appsList, parseRepoPath, repoPending, selectedChange, wipPreviewThreadId, threadMap } from '../../store/store';
import { CODING_AGENT_CHANNEL, type ThreadChannel, type InlineForm, type SettingsNavKey } from '../../store/store';
import { MENU_ITEM_LABELS } from '../../store/types';
import type { App } from '../../store/types';
import type { NavEntry } from '../../store/actions/navigation';
import { loadedOr } from '../../store/types';
import { formatChannel } from '../../utils/formatChannel';
import { previewFileName } from '../../utils/previewPath';
import { PENDING_TITLE_PLACEHOLDER } from '../../store/thread-events';

export const CHANNEL_OPTIONS: { value: ThreadChannel; label: string }[] = [
  { value: 'chat', label: formatChannel('chat') },
  { value: CODING_AGENT_CHANNEL, label: 'Coding Agent' },
  { value: 'trigger', label: formatChannel('trigger') },
];

function getFormTitle(form: InlineForm): string {
  switch (form.type) {
    case 'trigger': {
      if (!form.triggerId) return 'New Trigger';
      const trigger = loadedOr(triggers.value, []).find(t => t.id === form.triggerId);
      return trigger?.name || 'Trigger';
    }
    case 'app-edit': {
      const app = loadedOr(appsList.value, []).find(s => s.id === form.appId);
      return app?.name || 'Edit App';
    }
    case 'new-app': return 'New App';
    case 'credential': return form.editing ? 'Edit Credential' : 'Add Credential';
    case 'email-confirm': return form.sentAt ? 'Email Sent' : 'Confirm Email';
    case 'plugin-install':
      return `${form.installed ? 'Installed' : 'Install'} ${form.request.plugin_name}`;
    case 'plugin-uninstall':
      return `${form.removed ? 'Uninstalled' : 'Uninstall'} ${form.request.plugin_name}`;
  }
}

function getHostname(url: string): string {
  try { return new URL(url).hostname; } catch { return url; }
}

function appTitle(app: App, short: boolean): string {
  if (short || app.kind !== 'widget') return app.name;
  const origin = app.origin_thread_id ? threadMap.value.get(app.origin_thread_id)?.meta.title : undefined;
  if (!origin || origin === PENDING_TITLE_PLACEHOLDER) return `${app.name} (widget)`;
  return `${app.name} (widget from ${origin})`;
}

const TERMINAL_TITLE = 'Terminal';

function contentTitle(short: boolean): string {
  const settingsLabel: (key: SettingsNavKey) => string | undefined =
    short ? settingsSubviewShortLabel : settingsSubviewLabel;
  const overlay = panelOverlay.value;
  const url = panelUrl.value;
  const pageTitle = panelTitle.value;
  const active = activeMenuItem.value;

  if (overlay?.type === 'form') return getFormTitle(overlay.form);
  if (overlay?.type === 'app-ui') {
    const wipTid = wipPreviewThreadId.value;
    if (wipTid) {
      const wipTitle = threadMap.value.get(wipTid)?.meta.title;
      if (wipTitle && wipTitle !== PENDING_TITLE_PLACEHOLDER && !short) {
        return `${overlay.app.name} (WIP by ${wipTitle})`;
      }
      if (wipTitle) return `${overlay.app.name} (WIP)`;
    }
    return appTitle(overlay.app, short);
  }
  if (overlay?.type === 'file-preview') {
    const fileName = previewFileName(overlay.path);
    const desc = short ? null : getDiffDescription();
    if (desc) return `${fileName}: ${desc}`;
    return fileName;
  }
  if (overlay?.type === 'url-preview') return pageTitle || getHostname(url!);
  if (overlay?.type === 'notification-detail') {
    if (short || !overlay.notification.title) return 'Notification';
    return `Notification - ${overlay.notification.title}`;
  }
  if (overlay?.type === 'plugin-detail') return overlay.name;
  if (overlay?.type === 'terminal') return TERMINAL_TITLE;
  if (!overlay && active === 'settings' && settingsSubview.value !== 'main') {
    return settingsLabel(settingsSubview.value) || '';
  }
  return MENU_ITEM_LABELS[active] || '';
}

export function getContentTitle(): string {
  return contentTitle(false);
}

export function getContentTitleShort(): string {
  return contentTitle(true);
}

export function navEntryTitle(entry: NavEntry): string {
  const overlay = entry.overlay;
  if (overlay?.type === 'form') return getFormTitle(overlay.form);
  if (overlay?.type === 'app-ui') {
    const wip = entry.wipPreviewThreadId;
    if (wip) {
      const wipTitle = threadMap.value.get(wip)?.meta.title;
      if (wipTitle && wipTitle !== PENDING_TITLE_PLACEHOLDER) return `${overlay.app.name} (WIP by ${wipTitle})`;
      if (wipTitle) return `${overlay.app.name} (WIP)`;
    }
    return appTitle(overlay.app, false);
  }
  if (overlay?.type === 'file-preview') return previewFileName(overlay.path);
  if (overlay?.type === 'url-preview') return getHostname(overlay.url);
  if (overlay?.type === 'notification-detail') return overlay.notification.title || 'Notification';
  if (overlay?.type === 'plugin-detail') return overlay.name;
  if (overlay?.type === 'terminal') return TERMINAL_TITLE;
  if (!overlay && entry.menuItem === 'settings' && entry.settingsSubview !== 'main') {
    return settingsSubviewLabel(entry.settingsSubview) || 'Settings';
  }
  return MENU_ITEM_LABELS[entry.menuItem] || entry.menuItem;
}

export function navEntryCategory(entry: NavEntry): string {
  const overlay = entry.overlay;
  if (overlay?.type === 'form') {
    switch (overlay.form.type) {
      case 'trigger': return 'triggers';
      case 'app-edit':
      case 'new-app': return 'apps';
      case 'credential':
      case 'email-confirm': return 'settings';
      case 'plugin-install':
      case 'plugin-uninstall': return 'plugins';
    }
  }
  if (overlay?.type === 'app-ui') return 'apps';
  if (overlay?.type === 'file-preview') return 'files';
  if (overlay?.type === 'url-preview') return 'web';
  if (overlay?.type === 'notification-detail') return 'notifications';
  if (overlay?.type === 'plugin-detail') return 'plugins';
  if (overlay?.type === 'terminal') return 'terminal';
  return entry.menuItem;
}

export function getDiffDescription(): string | null {
  const overlay = panelOverlay.value;
  if (overlay?.type !== 'file-preview') return null;
  const parsed = parseRepoPath(overlay.path);
  if (parsed?.mode !== 'diff') return null;
  const desc = repoPending.value?.description
    ?? selectedChange.value?.description;
  return desc || null;
}
